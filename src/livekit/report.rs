//! Building the report packet the browser receives when an interview ends.
//!
//! One region because it is one output. The interview loop freezes the
//! assessment with `freeze_assessment`, runs `generate_report_bounded` beside
//! the farewell, and hands what came back to `publish_with_recovery`, which
//! publishes it or offers one regeneration first; everything below
//! is how the packet is assembled, and the pieces are separated so that a
//! failure in one of them is a note in the report rather than no report at
//! all.
//!
//! Split out of `livekit.rs` along the line its module doc already drew.

use ::livekit::prelude::{DataPacket, Room, RoomEvent};
use std::time::Duration;

use crate::agent::{
    ModelInputKind, ReportPromptInput, RuntimeState, final_report, format_test_run,
    framework_evidence_json, interview_contract_json, report_prompt, report_system_instruction,
    rolling_assessment, transcript_for_report,
};
use crate::gemini::{GeminiKeys, ReportMaterial, generate_report_with_keys};
use crate::runtime::{RuntimeBootstrap, TOPIC_REPORT};

use super::board::ReportBoard;
use super::{REPORT_TIMEOUT, browser_packet};

/// What the report call returned, or the deadline it missed.
pub(super) type GeneratedReport = Result<
    Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>>,
    tokio::time::error::Elapsed,
>;

/// The report prompt, built and counted once the interview's assessment is
/// over and before the farewell is spoken, so the call can run while it plays.
///
/// `board_attached` is whether a whiteboard image goes with it, which the
/// prompt has to know: told to grade a board that never arrived, the reviewer
/// goes looking for an attachment that is not there.
fn freeze_report_prompt(
    boot: &RuntimeBootstrap<'_>,
    state: &mut RuntimeState,
    elapsed_min: f64,
    board_attached: bool,
) -> String {
    let prompt = report_prompt_text(boot, state, elapsed_min, board_attached);

    // Counted with the system instruction it goes out behind, since the model
    // reads both.
    state.evidence_ledger.record_model_input(
        ModelInputKind::FinalReport,
        &format!(
            "{}\n\n{prompt}",
            report_system_instruction(boot.interview_mode)
        ),
    );
    prompt
}

pub(super) struct FrozenAssessment {
    pub prompt: String,
    /// The whiteboard images the prompt was frozen against, copied out so a
    /// regeneration sends the reviewer the same pictures as the first call.
    boards: Vec<ReportBoard>,
    state: RuntimeState,
    /// Whether the first generation had an answer refused, for recovery.
    pub refused: std::sync::atomic::AtomicBool,
}

impl FrozenAssessment {
    pub(super) fn drawing_received(&self) -> bool {
        self.state.board_snapshots > 0
    }

    pub(super) fn behavioral_round_opened(&self) -> bool {
        crate::agent::BehavioralRound::of(&self.state).opened()
    }

    pub(super) fn report_boards(&self) -> Vec<(&str, &[u8])> {
        labeled_boards(&self.boards)
    }
}

fn labeled_boards(boards: &[ReportBoard]) -> Vec<(&str, &[u8])> {
    boards
        .iter()
        .map(|board| (board.label, board.bytes.as_slice()))
        .collect()
}

pub(super) fn freeze_assessment(
    boot: &RuntimeBootstrap<'_>,
    state: &mut RuntimeState,
    elapsed_min: f64,
    boards: Vec<ReportBoard>,
) -> FrozenAssessment {
    let boards = if boot.interview_mode.is_whiteboard() {
        boards
    } else {
        Vec::new()
    };
    let prompt = freeze_report_prompt(boot, state, elapsed_min, !boards.is_empty());
    FrozenAssessment {
        prompt,
        boards,
        state: state.clone(),
        refused: std::sync::atomic::AtomicBool::new(false),
    }
}

/// The report call under `REPORT_TIMEOUT`. Borrows nothing of the interview
/// state, which is what lets it run beside the farewell that still needs it.
///
/// `boards` are the whiteboard phase checkpoints and final state. An editor
/// interview passes an empty slice; a whiteboard interview passes every image
/// that reached the agent, so clearing between phases does not erase evidence.
pub(super) async fn generate_report_bounded(
    boot: &RuntimeBootstrap<'_>,
    prompt: &str,
    boards: &[(&str, &[u8])],
    (behavioral_round_opened, drawing_received): (bool, bool),
    api_key: &GeminiKeys,
    seed: i64,
    refused: &std::sync::atomic::AtomicBool,
) -> GeneratedReport {
    tokio::time::timeout(
        REPORT_TIMEOUT,
        generate_report_with_keys(
            api_key,
            boot.report_model,
            prompt,
            ReportMaterial {
                mode: boot.interview_mode,
                drawing_received,
                boards,
            },
            boot.problem,
            behavioral_round_opened,
            crate::gemini::ReportRun {
                scope: boot.room_name,
                seed,
                refused,
            },
        ),
    )
    .await
}

pub(super) const DELIVERY_ATTEMPTS: usize = 3;
pub(super) const DELIVERY_WAIT: Duration = Duration::from_secs(5);

fn is_report_receipt(
    topic: Option<&str>,
    sender: Option<&str>,
    candidate: &str,
    payload: &[u8],
    id: &str,
    candidate_gone: bool,
) -> bool {
    // LiveKit resolves the sender against its current roster, so a receipt that
    // lands after the candidate's departure arrives with no participant. The
    // report is broadcast, so any peer could echo its digest; an unattributed
    // receipt counts only once the candidate is gone and no retry could reach
    // them anyway.
    topic == Some(crate::runtime::TOPIC_CONTROL)
        && (sender.is_none() && candidate_gone
            || super::is_interview_participant(sender, candidate))
        && serde_json::from_slice::<serde_json::Value>(payload)
            .is_ok_and(|value| value["type"] == "report_received" && value["deliveryId"] == id)
}

pub(super) trait DeliveryEvent {
    fn acknowledges(&self, candidate: &str, id: &str, candidate_gone: bool) -> bool;
    fn disconnected(&self) -> bool {
        false
    }
}

impl DeliveryEvent for RoomEvent {
    fn acknowledges(&self, candidate: &str, id: &str, candidate_gone: bool) -> bool {
        if let Self::DataReceived {
            payload,
            topic,
            participant,
            ..
        } = self
        {
            let sender = participant
                .as_ref()
                .map(|participant| participant.identity().0);
            is_report_receipt(
                topic.as_deref(),
                sender.as_deref(),
                candidate,
                payload,
                id,
                candidate_gone,
            )
        } else {
            false
        }
    }
    fn disconnected(&self) -> bool {
        matches!(self, Self::Disconnected { .. })
    }
}

/// A successful SDK publish only queues the packet. Keep the room alive until
/// the candidate acknowledges it, retransmitting the immutable bytes on loss.
/// Neither retries nor receipt timeouts call the report model again.
pub(super) async fn deliver_report<F, Fut, E, Event>(
    mut publish: F,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    candidate: &str,
    room_name: &str,
    candidate_present: impl Fn() -> bool,
    packet: DataPacket,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>
where
    F: FnMut(DataPacket) -> Fut,
    Fut: std::future::Future<Output = Result<(), E>>,
    Event: DeliveryEvent,
{
    let id = crate::sha256_hex(&[&packet.payload]);
    let mut published = false;
    for attempt in 1..=DELIVERY_ATTEMPTS {
        let deadline = tokio::time::Instant::now() + DELIVERY_WAIT;
        let failure = match tokio::time::timeout_at(deadline, publish(packet.clone())).await {
            Ok(Ok(())) => {
                published = true;
                None
            }
            Ok(Err(_)) => Some("publish_error"),
            Err(_) => Some("timeout"),
        };
        if let Some(cause) = failure {
            eprintln!(
                "codetrial report_publish_failed room={room_name} attempt={attempt} cause={cause}"
            );
        }

        // A failed publish waits out the rest of its window. The wait also
        // accepts a late receipt for an earlier attempt and observes
        // departures.
        let gone = |events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>| {
            if queued_receipt(events, candidate, &id) {
                Wait::Acknowledged
            } else {
                Wait::Gone
            }
        };
        let wait = if candidate_present() {
            tokio::time::timeout_at(deadline, async {
                while let Some(event) = events.recv().await {
                    // Presence first: the receipt that arrives unattributed
                    // because its sender just left is this very event.
                    let candidate_gone = event.disconnected() || !candidate_present();
                    if event.acknowledges(candidate, &id, candidate_gone) {
                        return Wait::Acknowledged;
                    }
                    if candidate_gone {
                        return gone(events);
                    }
                }
                Wait::Gone
            })
            .await
            .unwrap_or(Wait::TimedOut)
        } else {
            gone(events)
        };
        match wait {
            Wait::Acknowledged => {
                eprintln!(
                    "codetrial report_delivery room={room_name} attempt={attempt} outcome=acknowledged"
                );
                return Ok(true);
            }
            Wait::Gone => break,
            Wait::TimedOut => (),
        }
        if published {
            eprintln!("codetrial report_receipt_missing room={room_name} attempt={attempt}");
        }
    }
    if published {
        eprintln!("codetrial report_delivery room={room_name} outcome=unconfirmed");
        Ok(false)
    } else {
        eprintln!("codetrial report_delivery room={room_name} outcome=failed");
        Err("report_delivery_failed: all publication attempts failed or timed out".into())
    }
}

/// How one attempt's receipt wait ended. `Gone` covers the candidate leaving,
/// the room disconnecting and the event stream closing: nobody is left to
/// retransmit to.
enum Wait {
    Acknowledged,
    Gone,
    TimedOut,
}

/// The roster drops the candidate when LiveKit handles the departure, not when
/// this loop reads the queue, so a receipt the page sent just before leaving
/// can still be waiting behind events the loop has not reached.
fn queued_receipt<Event: DeliveryEvent>(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    candidate: &str,
    id: &str,
) -> bool {
    while let Ok(event) = events.try_recv() {
        if event.acknowledges(candidate, id, true) {
            return true;
        }
    }
    false
}

const REPORT_RECOVERY_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);
const REPORT_RETRY_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);

/// Whether the generation ended on a refused answer, rather than on a call
/// that never answered.
fn refused_report(generated: &GeneratedReport) -> bool {
    matches!(generated, Ok(Err(error)) if crate::gemini::is_report_schema_failure(error.as_ref()))
}

/// The seed the one regeneration asks with, given whether the first
/// generation had an answer refused, for the reasons `REGENERATION_SEED`
/// gives.
fn regeneration_seed(refused: bool) -> i64 {
    if refused {
        crate::gemini::REGENERATION_SEED
    } else {
        crate::gemini::GENERATION_SEED
    }
}

/// How long the candidate waits before a regeneration may start, or `None`
/// when the failure is not one a regeneration can fix. A deadline and a refused
/// report say nothing about the keys, so both ask the rotation itself: a retry
/// offered while every key is still out on quota would fail before its first
/// call and spend the one retry.
fn regeneration_cooldown(
    generated: &GeneratedReport,
    keys: &GeminiKeys,
) -> Option<std::time::Duration> {
    let retry_after = match generated {
        Ok(Ok(_)) => None,
        Ok(Err(error)) if !crate::gemini::is_report_schema_failure(error.as_ref()) => {
            crate::gemini::report_regeneration_retry_after(error.as_ref())
        }
        _ => match report_readiness(keys) {
            Readiness::Ready => Some(std::time::Duration::ZERO),
            Readiness::Wait(delay) => Some(delay),
            Readiness::Never => None,
        },
    };
    retry_after.map(|delay| delay.max(REPORT_RETRY_COOLDOWN))
}

/// Whether the key rotation can make a report call now, and if not, whether
/// waiting would help. Another interview sharing the keys can put them back on
/// quota at any moment, so this is asked again when a retry arrives rather than
/// trusted from when the offer went out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Readiness {
    Ready,
    Wait(std::time::Duration),
    Never,
}

fn report_readiness(keys: &GeminiKeys) -> Readiness {
    match keys.select_report() {
        Ok(_) => Readiness::Ready,
        Err(error) => match crate::gemini::report_regeneration_retry_after(&error) {
            Some(delay) => Readiness::Wait(delay),
            None => Readiness::Never,
        },
    }
}

fn recovery_request(
    topic: Option<&str>,
    sender: Option<&str>,
    candidate: &str,
    payload: &[u8],
) -> bool {
    super::interview_packet(topic, sender, candidate, payload).is_some_and(|(topic, payload)| {
        topic == crate::runtime::TOPIC_CONTROL && payload["type"] == "retry_report"
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryEvent {
    Retry,
    /// The room itself is gone, so nothing published can arrive.
    Left,
    /// The candidate left, which a full LiveKit rejoin under the same identity
    /// also looks like until `Back`.
    Away,
    Back,
    Ignore,
}

/// How long a candidate who left may take to rejoin before the wait gives up.
/// A full LiveKit rejoin after a network drop leaves and returns under the same
/// identity, and treating the leave as final spent the one retry on a candidate
/// who never went anywhere.
const REJOIN_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

/// The agent's answer to a retry, and its word that the window closed. Without
/// them the page had to guess both from its own clock, which starts later than
/// this one and stops for a reconnect this one never sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryNotice {
    Accepted,
    Early(std::time::Duration),
    Closed,
}

/// A wait as the page is told it, rounded up, so a page that waits exactly
/// this long is not early.
pub(super) fn whole_seconds(wait: std::time::Duration) -> u64 {
    wait.as_millis().div_ceil(1000) as u64
}

fn recovery_notice(notice: RecoveryNotice) -> serde_json::Value {
    match notice {
        RecoveryNotice::Accepted => {
            serde_json::json!({ "type": "report_retry", "status": "accepted" })
        }
        RecoveryNotice::Early(wait) => serde_json::json!({
            "type": "report_retry",
            "status": "early",

            "retryAfterSeconds": whole_seconds(wait).max(1),
        }),
        RecoveryNotice::Closed => serde_json::json!({ "type": "report_retry", "status": "closed" }),
    }
}

/// The room as recovery uses it: what arrives, and what goes back out. A
/// separate seam from the room itself so the wait can be driven by a script.
trait RecoveryRoom {
    /// Whether the candidate is in the room now. Their departure is an event
    /// the interview loop may already have consumed, and a wait for a retry
    /// from nobody held the slot for the whole window.
    fn candidate_present(&self) -> bool;
    fn next(&mut self) -> impl std::future::Future<Output = RecoveryEvent> + Send;
    fn notify(&mut self, notice: RecoveryNotice) -> impl std::future::Future<Output = ()> + Send;
    /// `Ok(true)` once the page acknowledged the packet, `Ok(false)` when it
    /// went out unconfirmed.
    fn publish(
        &mut self,
        packet: DataPacket,
    ) -> impl std::future::Future<Output = Result<bool, Box<dyn std::error::Error + Send + Sync>>> + Send;
}

struct LiveRecoveryRoom<'a> {
    room: &'a Room,
    room_name: &'a str,
    candidate: &'a str,
    events: &'a mut tokio::sync::mpsc::UnboundedReceiver<::livekit::RoomEvent>,
}

/// What one room event means to a recovery wait. `None` is the event stream
/// ending, which only happens once the room is gone.
fn recovery_event(event: Option<::livekit::RoomEvent>, candidate: &str) -> RecoveryEvent {
    match event {
        None | Some(::livekit::RoomEvent::Disconnected { .. }) => RecoveryEvent::Left,
        Some(::livekit::RoomEvent::ParticipantDisconnected(p)) => {
            candidate_only(&p.identity().0, candidate, RecoveryEvent::Away)
        }
        Some(::livekit::RoomEvent::ParticipantConnected(p)) => {
            candidate_only(&p.identity().0, candidate, RecoveryEvent::Back)
        }
        Some(::livekit::RoomEvent::DataReceived {
            topic,
            payload,
            participant,
            ..
        }) => {
            let sender = participant.as_ref().map(|p| p.identity().0);
            if recovery_request(topic.as_deref(), sender.as_deref(), candidate, &payload) {
                RecoveryEvent::Retry
            } else {
                RecoveryEvent::Ignore
            }
        }
        _ => RecoveryEvent::Ignore,
    }
}

/// Only the candidate coming and going matters. Anyone else, an observer or
/// the recording egress, says nothing about whether a retry can come.
fn candidate_only(identity: &str, candidate: &str, event: RecoveryEvent) -> RecoveryEvent {
    if identity == candidate {
        event
    } else {
        RecoveryEvent::Ignore
    }
}

impl RecoveryRoom for LiveRecoveryRoom<'_> {
    fn candidate_present(&self) -> bool {
        self.room
            .remote_participants()
            .contains_key(&::livekit::id::ParticipantIdentity(
                self.candidate.to_string(),
            ))
    }

    async fn next(&mut self) -> RecoveryEvent {
        recovery_event(self.events.recv().await, self.candidate)
    }

    async fn notify(&mut self, notice: RecoveryNotice) {
        // A notice that cannot be sent leaves the page on its own fallback
        // timers, which is no worse than before notices existed, so it is
        // published once and never waits for a receipt.
        let sent: Result<(), Box<dyn std::error::Error + Send + Sync>> =
            match browser_packet(crate::runtime::TOPIC_CONTROL, &recovery_notice(notice)) {
                Ok(packet) => self
                    .room
                    .local_participant()
                    .publish_data(packet)
                    .await
                    .map_err(Into::into),
                Err(error) => Err(error.into()),
            };
        if let Err(error) = sent {
            eprintln!("codetrial report_recovery_notice_failed notice={notice:?} error={error}");
        }
    }

    /// Reports only. Retry requests and departures that arrive during the
    /// receipt wait are consumed by it; `recover_report` reads presence from
    /// the room again when its own wait begins and after each republish.
    async fn publish(
        &mut self,
        packet: DataPacket,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let Self {
            room,
            room_name,
            candidate,
            events,
        } = self;
        let participant = room.local_participant();
        let identity = ::livekit::id::ParticipantIdentity(candidate.to_string());
        deliver_report(
            |packet| participant.publish_data(packet),
            events,
            candidate,
            room_name,
            || room.remote_participants().contains_key(&identity),
            packet,
        )
        .await
    }
}

/// Sleeps until an absent candidate's rejoin grace runs out, or forever while
/// they are present.
async fn rejoin_expired(presence: &super::CandidatePresence) {
    match presence.deadline(REJOIN_GRACE) {
        Some(deadline) => {
            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
        }
        None => std::future::pending().await,
    }
}

/// Follows the candidate's comings and goings, `Break` once the room itself is
/// gone. A retry can only come from the candidate, so it shows them present.
fn track_presence(
    event: RecoveryEvent,
    presence: &mut super::CandidatePresence,
) -> std::ops::ControlFlow<()> {
    match event {
        RecoveryEvent::Left => return std::ops::ControlFlow::Break(()),

        // The tokio clock, read as a std instant, so a paused test clock drives
        // the grace the same way the real one does.
        RecoveryEvent::Away => presence.left(tokio::time::Instant::now().into_std()),
        RecoveryEvent::Back | RecoveryEvent::Retry => presence.returned(),
        RecoveryEvent::Ignore => {}
    }
    std::ops::ControlFlow::Continue(())
}

/// `unconfirmed` is the provisional report when no receipt came back for it.
/// The candidate may have dropped before it arrived, so a rejoin republishes
/// it; without that the rejoined page never learns a retry is on offer.
async fn recover_report(
    events: &mut impl RecoveryRoom,
    mut unconfirmed: Option<DataPacket>,
    generation: impl std::future::Future<Output = GeneratedReport>,
    window: std::time::Duration,
    cooldown: std::time::Duration,
    started: tokio::time::Instant,
    readiness: impl Fn() -> Readiness,
) -> Option<GeneratedReport> {
    // A departure the provisional report's receipt wait consumed is not
    // replayed, so absence starts the rejoin grace here instead of the wait
    // holding the slot for a retry from nobody.
    let mut presence = super::CandidatePresence::default();
    if !events.candidate_present() {
        presence.left(tokio::time::Instant::now().into_std());
    }
    loop {
        let event = tokio::select! {
            biased;
            _ = tokio::time::sleep_until(started + window) => {
                events.notify(RecoveryNotice::Closed).await;
                return None;
            }
            _ = rejoin_expired(&presence) => return None,
            event = events.next() => event,
        };
        if track_presence(event, &mut presence).is_break() {
            return None;
        }
        if event == RecoveryEvent::Back
            && let Some(packet) = unconfirmed.take()
        {
            match events.publish(packet.clone()).await {
                Ok(true) => {}
                Ok(false) => unconfirmed = Some(packet),
                Err(error) => {
                    eprintln!("codetrial report_republish_failed error={error}");
                    unconfirmed = Some(packet);
                }
            }
            // The delivery read the events a departure would have arrived on.
            if !events.candidate_present() {
                presence.left(tokio::time::Instant::now().into_std());
            }
            continue;
        }
        let RecoveryEvent::Retry = event else {
            continue;
        };
        // Only a page holding the provisional report can ask for a retry.
        unconfirmed = None;
        let waited = started.elapsed();
        if waited < cooldown {
            events
                .notify(RecoveryNotice::Early(cooldown - waited))
                .await;
            continue;
        }
        match readiness() {
            Readiness::Ready => {
                events.notify(RecoveryNotice::Accepted).await;
                break;
            }
            Readiness::Wait(delay) => events.notify(RecoveryNotice::Early(delay)).await,
            Readiness::Never => {
                events.notify(RecoveryNotice::Closed).await;
                return None;
            }
        }
    }

    // The generation keeps running while the candidate is away within the
    // grace, and what it returns waits for them to be back before it goes out.
    tokio::pin!(generation);
    let mut generated = None;
    loop {
        if presence.deadline(REJOIN_GRACE).is_none()
            && let Some(result) = generated.take()
        {
            return Some(result);
        }
        tokio::select! {
            _ = rejoin_expired(&presence) => return None,
            event = events.next() => {
                if track_presence(event, &mut presence).is_break() {
                    return None;
                }
            }
            result = &mut generation, if generated.is_none() => generated = Some(result),
        }
    }
}

pub(super) struct ReportRecovery<'a> {
    pub boot: &'a RuntimeBootstrap<'a>,
    pub assessment: FrozenAssessment,
    pub reason: &'a str,
    pub keys: &'a GeminiKeys,
    pub candidate: &'a str,
}

/// Recovery runs a separate event loop: the ended interview must never pump
/// new media into Gemini or interpret its deliberate shutdown as a reconnect.
pub(super) async fn publish_with_recovery(
    recovery: ReportRecovery<'_>,
    generated: GeneratedReport,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<::livekit::RoomEvent>,
    room: &Room,
    close_live: impl std::future::Future<Output = ()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let ReportRecovery {
        boot,
        assessment,
        reason,
        keys,
        candidate,
    } = recovery;
    let FrozenAssessment {
        prompt,
        boards,
        mut state,
        refused,
    } = assessment;
    let refused = refused.into_inner() || refused_report(&generated);
    let again = std::sync::atomic::AtomicBool::new(false);
    let behavioral_round_opened = crate::agent::BehavioralRound::of(&state).opened();
    let drawing_received = state.board_snapshots > 0;
    let boards = labeled_boards(&boards);
    let mut room = LiveRecoveryRoom {
        room,
        room_name: boot.room_name,
        candidate,
        events,
    };
    run_recovery(
        &mut room,
        RecoveryReport {
            boot,
            state: &mut state,
            reason,
            keys,
            refused,
        },
        generated,
        generate_report_bounded(
            boot,
            &prompt,
            &boards,
            (behavioral_round_opened, drawing_received),
            keys,
            regeneration_seed(refused),
            &again,
        ),
        tokio::time::Instant::now,
        close_live,
    )
    .await
}

/// What each published report is built from. The frozen state, never the live
/// one, so a regenerated report carries the same evidence as the failure it
/// replaces.
struct RecoveryReport<'a> {
    boot: &'a RuntimeBootstrap<'a>,
    state: &'a mut RuntimeState,
    reason: &'a str,
    keys: &'a GeminiKeys,
    /// Whether the first generation had an answer refused, which a deadline
    /// that overtook it no longer says.
    refused: bool,
}

/// `clock` names when the offer went out. The agent reads the real clock; a
/// test reads one already past the cooldown rather than pausing time under a
/// real HTTP exchange. `close_live` runs exactly once, on every path.
async fn run_recovery(
    room: &mut impl RecoveryRoom,
    report: RecoveryReport<'_>,
    generated: GeneratedReport,
    regenerate: impl std::future::Future<Output = GeneratedReport>,
    clock: fn() -> tokio::time::Instant,
    close_live: impl std::future::Future<Output = ()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let RecoveryReport {
        boot,
        state,
        reason,
        keys,
        refused,
    } = report;

    // The Live session is closed once, beside the first report's delivery:
    // waiting for it would delay the report by up to its close timeout, and
    // closing after it would hold Live open through the receipt retries and a
    // recovery wait of minutes.
    let Some(cooldown) =
        regeneration_cooldown(&generated, keys).filter(|_| room.candidate_present())
    else {
        let (published, ()) = tokio::join!(
            async {
                room.publish(report_packet(boot, state, reason, keys, generated)?)
                    .await
            },
            close_live
        );
        return published.map(|_| ());
    };
    let cause = if refused { "schema" } else { "unavailable" };
    let mut provisional = report_value(boot, state, reason, keys, generated);
    provisional["reportRecovery"] = recovery_metadata(cooldown, cause);
    let provisional = report_data_packet(provisional)?;
    let started = clock();
    let (acknowledged, ()) = tokio::join!(room.publish(provisional.clone()), close_live);
    let unconfirmed = (!acknowledged?).then_some(provisional);
    if let Some(generated) = recover_report(
        room,
        unconfirmed,
        regenerate,
        REPORT_RECOVERY_WINDOW,
        cooldown,
        started,
        || report_readiness(keys),
    )
    .await
    {
        room.publish(report_packet(boot, state, reason, keys, generated)?)
            .await?;
    }
    Ok(())
}

/// `cause` is `schema` when the report answered and was refused, so the page
/// does not call it a passing outage, and `unavailable` otherwise.
fn recovery_metadata(cooldown: std::time::Duration, cause: &str) -> serde_json::Value {
    serde_json::json!({
        "expiresInSeconds": REPORT_RECOVERY_WINDOW.as_secs(),
        "retryAfterSeconds": whole_seconds(cooldown),
        "cause": cause,
    })
}

fn report_packet(
    boot: &RuntimeBootstrap<'_>,
    state: &mut RuntimeState,
    reason: &str,
    keys: &GeminiKeys,
    generated: GeneratedReport,
) -> Result<DataPacket, Box<dyn std::error::Error + Send + Sync>> {
    Ok(report_data_packet(report_value(
        boot, state, reason, keys, generated,
    ))?)
}

fn report_value(
    boot: &RuntimeBootstrap<'_>,
    state: &mut RuntimeState,
    reason: &str,
    api_key: &GeminiKeys,
    generated: GeneratedReport,
) -> serde_json::Value {
    let mut report = match generated {
        Ok(Ok(raw)) => final_report(Some(&raw), state.hints_used, None, boot.problem),
        Ok(Err(error)) => final_report(
            None,
            state.hints_used,
            Some(&report_error_note(
                boot,
                state,
                reason,
                error.as_ref(),
                api_key,
            )),
            boot.problem,
        ),

        // `Elapsed` Displays as "deadline has elapsed", which does not say
        // whose deadline. Not "Gemini did not answer" either: the deadline
        // covers the repairs and the retry waits too, so it also runs out on a
        // Gemini that answered every time with something unusable.
        Err(_) => final_report(
            None,
            state.hints_used,
            Some(&report_error_note(
                boot,
                state,
                reason,
                &std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!(
                        "Report generation did not finish within {}s",
                        REPORT_TIMEOUT.as_secs()
                    ),
                ),
                api_key,
            )),
            boot.problem,
        ),
    };
    stamp_report_debrief(&mut report, boot, state);
    stamp_report_contract(&mut report);
    report_with_integrity_events(report, state, reason)
}

/// The teaching material that becomes useful only after an interview ends.
///
/// Every field here is bank-validated before it gets this far: the scenario
/// contract, hints and follow-ups always were, and `optimal` and `pitfalls`
/// joined them when `posed` started renaming them and `check_source_absent`
/// started reading them. This filter is the backstop behind that, not the
/// thing standing between a published name and a candidate, which is why it
/// drops a field rather than refusing the report.
fn stamp_report_debrief(
    report: &mut serde_json::Value,
    boot: &RuntimeBootstrap<'_>,
    state: &RuntimeState,
) {
    let problem = boot.problem;

    // The empty title an original problem gives matches no title but still
    // refuses a practice site, which is what `validate_report_candidate`
    // already does. Skipping the check for those problems instead would leave
    // the one kind of exercise this filter cannot speak for.
    let source_title = problem.source_title().unwrap_or("");
    let safe = |field: &str, text: &str| {
        if crate::agent::names_published_problem(source_title, text) {
            eprintln!(
                "codetrial report_debrief_dropped problem={} field={field}",
                problem.id
            );
            None
        } else {
            Some(text.to_string())
        }
    };
    let variant = problem.variant();
    let hints = variant
        .hints
        .iter()
        .enumerate()
        .filter_map(|(index, hint)| {
            safe("hints", hint).map(|text| {
                serde_json::json!({
                    "text": text,
                    "given": index < state.hint_rungs_given,
                })
            })
        })
        .collect::<Vec<_>>();
    let follow_ups = variant
        .follow_ups
        .iter()
        .filter_map(|follow_up| safe("followUps", follow_up))
        .collect::<Vec<_>>();
    let debrief = serde_json::json!({
        "scenarioContract": safe("scenarioContract", variant.contract),
        "approach": safe("approach", problem.optimal),
        "pitfalls": safe("pitfalls", problem.pitfalls),
        "hints": hints,
        "followUps": follow_ups,
    });
    if let Some(object) = report.as_object_mut() {
        object.insert("debrief".to_string(), debrief);
        object.insert(
            "topics".to_string(),
            serde_json::json!(crate::agent::topics_for(problem.id).unwrap_or(&[])),
        );
        object.insert(
            "practiceLevel".to_string(),
            serde_json::json!(boot.profile.seniority.map(crate::agent::Seniority::as_str)),
        );
    }
}

fn stamp_report_contract(report: &mut serde_json::Value) {
    if let Some(object) = report.as_object_mut() {
        object.insert("interviewContract".to_string(), interview_contract_json());
    }
}

fn report_with_integrity_events(
    mut report: serde_json::Value,
    state: &RuntimeState,
    reason: &str,
) -> serde_json::Value {
    if let Some(object) = report.as_object_mut() {
        // The evidence, plus the heartbeats that bookend it, merged by sequence
        // rather than appended: a reader goes down this list in the order the
        // interview happened, and a device-state sample out of place reads as a
        // fault rather than as a bookend.
        let kept = state.integrity_events.len() as u64;
        let liveness = state.integrity_first_heartbeat.iter();
        let liveness = liveness.chain(state.integrity_last_heartbeat.iter());
        let samples = liveness.clone().count() as u64;
        let mut events = state.integrity_events.clone();
        events.extend(liveness.cloned());
        events.sort_by_key(|event| event["seq"].as_u64().unwrap_or(0));
        object.insert(
            "integrityEvents".to_string(),
            serde_json::Value::Array(events),
        );

        // How far verification got, and how much of it this report is not
        // showing. The array above is a subsequence, so its links cannot be
        // recomputed by whoever holds the report; without these a reader cannot
        // tell a retention gap from a deleted row, which is the distinction the
        // chain exists to make visible.
        //
        // The dropped count is derived rather than tallied. Every accepted
        // event is in the evidence, held as a sample, or gone, and the cursor
        // counts acceptances, so a counter would have been a fourth place for
        // the same fact to be wrong.
        let verified = state.integrity_chain.as_ref().map(|(seq, _)| *seq);
        object.insert("integrityChainSeq".to_string(), serde_json::json!(verified));
        object.insert(
            "integrityDropped".to_string(),
            serde_json::json!(verified.map(|seq| seq.saturating_sub(kept + samples))),
        );
        object.insert(
            "frameworkEvidence".to_string(),
            serde_json::Value::Array(
                state
                    .framework_evidence
                    .iter()
                    .map(framework_evidence_json)
                    .collect(),
            ),
        );
        let coding_gate = crate::agent::coding_round_complete(state);
        object.insert(
            "interviewLoop".to_string(),
            serde_json::json!(state.interview_loop.as_str()),
        );
        if state.code_execution_disabled {
            object.insert("codeExecution".to_string(), serde_json::Value::Bool(false));
        } else {
            object.remove("codeExecution");
        }

        // Which surface it was held on, beside the loop it was held in. The
        // card and the export both say it, and the saved report is the only
        // record of it once the room is gone: a whiteboard session otherwise
        // reads afterwards as an editor interview whose candidate typed
        // nothing.
        object.insert(
            "interviewMode".to_string(),
            serde_json::json!(state.interview_mode.as_str()),
        );

        // Why the interview ended, from the side that ended it. The page can
        // see that a report arrived unasked but not which clock produced it,
        // and it was deriving the answer from its own countdown: an interview
        // the interviewer closed after a suspended tab drifted past its
        // deadline was then recorded as having run out of time.
        object.insert("endReason".to_string(), serde_json::json!(reason));
        let star_complete = state.behavioral_round_started
            && crate::agent::phases_evidenced(
                state,
                &[
                    crate::agent::FrameworkPhase::Situation,
                    crate::agent::FrameworkPhase::Task,
                    crate::agent::FrameworkPhase::Action,
                    crate::agent::FrameworkPhase::Result,
                ],
            );
        object.insert("rounds".to_string(), serde_json::json!([
            {"kind":"coding","budgetMin": state.coding_minutes, "status": if coding_gate { "complete" } else { "incomplete" }},
            {"kind":"behavioral","budgetMin": state.behavioral_minutes, "status": if state.interview_loop == crate::agent::InterviewLoop::CodingOnly { "not_configured" } else if star_complete { "complete" } else if state.behavioral_round_started { "started" } else { "skipped" }}
        ]));
    }
    report
}

fn report_prompt_text(
    boot: &RuntimeBootstrap<'_>,
    state: &RuntimeState,
    elapsed_min: f64,
    board_attached: bool,
) -> String {
    let rolling = rolling_assessment(&state.framework_evidence, &state.interim_notes);

    // Passed apart from the rolling assessment, which the report prompt wraps
    // as untrusted material; the prompt owns the ledger's heading and its
    // place.
    let evidence = if state.evidence_ledger.entries.is_empty() && !state.code_execution_disabled {
        String::new()
    } else {
        state
            .prompt_evidence(crate::agent::ViewFor::Report)
            .join("\n")
    };
    let transcript = transcript_for_report(&crate::agent::report_transcript_lines(state));
    let test_summary = if state.code_execution_disabled {
        "Code execution was disabled by the candidate; testing evidence is the candidate's hand trace of written code, not executed cases.".to_string()
    } else {
        format_test_run(state.last_test_run.as_ref(), state.test_runs)
    };
    report_prompt(ReportPromptInput {
        problem: boot.problem,
        interview_mode: boot.interview_mode,
        board_attached,
        transcript: &transcript,
        rolling_assessment: &rolling,
        final_code: &state.code,
        language: &state.language,
        hints_used: state.hints_used,
        hint_rung: state.hint_rungs_given,
        volunteered_hints: state.volunteered_hints,
        duration_min: boot.duration_min,
        elapsed_min,
        test_summary: &test_summary,
        practice_level: boot.profile.seniority.map(crate::agent::Seniority::as_str),
        evidence: &evidence,
        behavioral_round: crate::agent::BehavioralRound::of(state),
    })
}

/// This note is published to the candidate's browser and rendered in the report
/// card, so `api_key` is not decoration: an error carrying a credentialed URL
/// would otherwise hand the server's Google key to whoever is taking the
/// interview.
///
/// The note is the cause and nothing else. The runner's exit reason, the model,
/// the problem id and the size of the editor used to lead it, so a candidate
/// met a sentence about a "Rust LiveKit runner" before the 503 that was the
/// whole story, and none of it was anything they could act on. It is what an
/// operator correlates on, so it goes to the log beside the cause.
fn report_error_note(
    boot: &RuntimeBootstrap<'_>,
    state: &RuntimeState,
    reason: &str,
    error: &(dyn std::error::Error + 'static),
    api_key: &GeminiKeys,
) -> String {
    let detail = api_key.redact(&error.to_string());
    eprintln!(
        "codetrial report_failed room={} runner_ended={reason:?} model={} problem={} editor_bytes={} language={} error={detail:?}",
        boot.room_name,
        boot.report_model,
        boot.problem.id,
        state.code.len(),
        state.language
    );
    detail
}

fn report_data_packet(report: serde_json::Value) -> Result<DataPacket, serde_json::Error> {
    browser_packet(TOPIC_REPORT, &report)
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/report.rs"]
mod tests;

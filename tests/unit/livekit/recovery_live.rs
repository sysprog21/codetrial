//! Credentialed fixture for the real LiveKit and Gemini crossings of the
//! recovery work, and the offline pieces it stands on.
//!
//! STATUS: THE LIVE TEST IN THIS FILE HAS NOT BEEN RUN. Nothing here was
//! observed against LiveKit, Gemini, the SDK data channel or a real room.
//! `live_sdk_control_presence_and_report_delivery` is `#[ignore]`d and wired:
//! it hosts the real room loop, a real SDK candidate and the observers, every
//! effect behind a permit, one scenario per run. It compiles and refuses
//! without the budget variables, and no run of it has taken a permit. What
//! does run is local: the permit client against the budget launcher's
//! protocol, the four declared action lists and their ceilings, the manifest
//! duration rules, the hard-deadline arithmetic, the candidate's report receipt
//! over a recorded room, the prepared-write gate seam, the reader-ending
//! trigger on a local socket, the observer, the receipt count and the verdict.
//! None of that certifies a live crossing.

use ::livekit::prelude::DataPacket;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

// The permit client: the native half of scripts/interview-run-budget.py's
// protocol, mirroring `WorkerGuard` there and `RunBudget` in
// scripts/run-budget-guard.cjs.

const PROTOCOL_VERSION: u64 = 1;
const PERMIT_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_REPLY_BYTES: usize = 4096;
/// The reserved finalization permit. The manifest does not list it.
const FINAL_ACTION_ID: &str = "end";

/// Why no permit was granted. Every variant is a refusal to act.
#[derive(Debug, PartialEq)]
enum PermitError {
    /// The B0.1 variables or the manifest are unusable.
    Setup(String),
    /// The launcher answered and the answer was not an exact grant.
    Denied(String),
    /// The launcher could not be reached, or did not answer in time. The
    /// request may have been granted, so nothing is retried.
    Lost(String),
    /// An earlier failure left this client unable to ask again.
    Dead(String),
}

impl std::fmt::Display for PermitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Setup(why) | Self::Denied(why) | Self::Lost(why) | Self::Dead(why) => {
                f.write_str(why)
            }
        }
    }
}

/// One request, one connection, one deadline for the whole exchange. A reply
/// is the first line, at most `MAX_REPLY_BYTES`, and a JSON object whose `v`
/// is the protocol version.
async fn exchange(socket: &Path, request: &Value) -> Result<Value, PermitError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let line = {
        let mut line = serde_json::to_vec(request).expect("a request is JSON");
        line.push(b'\n');
        line
    };
    let attempt = async {
        let mut stream = tokio::net::UnixStream::connect(socket)
            .await
            .map_err(|error| {
                PermitError::Lost(format!("launcher unreachable: {:?}", error.kind()))
            })?;
        stream.write_all(&line).await.map_err(|error| {
            PermitError::Lost(format!("launcher unreachable: {:?}", error.kind()))
        })?;
        let mut data = Vec::new();
        while !data.contains(&b'\n') && data.len() < MAX_REPLY_BYTES {
            let mut block = [0u8; 1024];
            let read = stream.read(&mut block).await.map_err(|error| {
                PermitError::Lost(format!("launcher unreachable: {:?}", error.kind()))
            })?;
            if read == 0 {
                return Err(PermitError::Lost("connection closed before a reply".into()));
            }
            data.extend_from_slice(&block[..read]);
        }
        Ok(data)
    };
    let data = tokio::time::timeout(PERMIT_TIMEOUT, attempt)
        .await
        .map_err(|_| PermitError::Lost("no reply within the permit timeout".into()))??;
    let first = data.split(|byte| *byte == b'\n').next().unwrap_or_default();
    let malformed = || PermitError::Denied("malformed reply".into());
    let reply: Value = serde_json::from_slice(first).map_err(|_| malformed())?;
    if !reply.is_object() || reply["v"].as_u64() != Some(PROTOCOL_VERSION) {
        return Err(malformed());
    }
    Ok(reply)
}

/// The B0.1 variables a native worker is started with. The launcher sets them
/// for its child; nothing else should.
const BUDGET_VARIABLES: [&str; 4] = [
    "BROWSER_CHECK_RUN_MANIFEST",
    "BROWSER_CHECK_RUN_ID",
    "BROWSER_CHECK_RUN_MANIFEST_SHA256",
    "BROWSER_CHECK_BUDGET_SOCKET",
];

struct PermitClient {
    socket: PathBuf,
    run_id: String,
    digest: String,
    manifest: Value,
    stop_file: Option<PathBuf>,
    next_seq: u64,
    granted: Vec<String>,
    dead: Option<String>,
}

impl PermitClient {
    async fn from_env() -> Result<Self, PermitError> {
        Self::from_variables(|name| std::env::var(name).ok()).await
    }

    /// Joins the run at the launcher's count, as a restarted worker does. A
    /// run with only some of the variables is refused, not run unbudgeted:
    /// this fixture exists to spend money, and only under a budget.
    async fn from_variables(get: impl Fn(&str) -> Option<String>) -> Result<Self, PermitError> {
        let missing: Vec<_> = BUDGET_VARIABLES
            .iter()
            .filter(|name| get(name).is_none_or(|value| value.is_empty()))
            .collect();
        if !missing.is_empty() {
            return Err(PermitError::Setup(format!(
                "run budget is incomplete, missing {missing:?}"
            )));
        }
        let value = |name: &str| get(name).expect("checked above");
        let raw = std::fs::read(value("BROWSER_CHECK_RUN_MANIFEST")).map_err(|error| {
            PermitError::Setup(format!("cannot read the run manifest: {:?}", error.kind()))
        })?;
        let digest = crate::sha256_hex(&[&raw]);
        if digest != value("BROWSER_CHECK_RUN_MANIFEST_SHA256") {
            return Err(PermitError::Setup(
                "manifest hash does not match the launcher's".into(),
            ));
        }
        let manifest: Value = serde_json::from_slice(&raw)
            .map_err(|_| PermitError::Setup("the run manifest is not JSON".into()))?;
        if manifest["run_id"] != value("BROWSER_CHECK_RUN_ID") {
            return Err(PermitError::Setup(
                "run ID does not match the manifest".into(),
            ));
        }
        let mut client = Self {
            socket: PathBuf::from(value("BROWSER_CHECK_BUDGET_SOCKET")),
            run_id: value("BROWSER_CHECK_RUN_ID"),
            digest,
            manifest,
            stop_file: get("BROWSER_CHECK_BUDGET_STOP").map(PathBuf::from),
            next_seq: 0,
            granted: Vec::new(),
            dead: None,
        };
        let status = client.call(json!({"op": "status"})).await?;
        match (status["ok"].as_bool(), status["count"].as_u64()) {
            (Some(true), Some(count)) => client.next_seq = count + 1,
            _ => {
                client.dead = Some("unusable status reply".into());
                return Err(PermitError::Denied("unusable status reply".into()));
            }
        }
        Ok(client)
    }

    async fn call(&self, mut request: Value) -> Result<Value, PermitError> {
        request["v"] = json!(PROTOCOL_VERSION);
        request["run_id"] = json!(self.run_id);
        request["manifest_sha256"] = json!(self.digest);
        exchange(&self.socket, &request).await
    }

    /// Whether the launcher has asked the worker to stop.
    fn stop_requested(&self) -> bool {
        self.stop_file.as_deref().is_some_and(Path::exists)
    }

    /// Asks for one action. Anything short of an exact grant for this action
    /// and sequence leaves the client dead: a request whose reply was lost may
    /// have been granted, and acting on a guess is how a budget is overrun.
    async fn permit(&mut self, action_id: &str) -> Result<(), PermitError> {
        if let Some(why) = &self.dead {
            return Err(PermitError::Dead(format!("guard is dead: {why}")));
        }
        let outcome = self.ask(action_id).await;
        match outcome {
            Ok(()) => {
                self.granted.push(action_id.to_owned());
                self.next_seq += 1;
                Ok(())
            }
            Err(error) => {
                self.dead = Some(error.to_string());
                Err(error)
            }
        }
    }

    async fn ask(&self, action_id: &str) -> Result<(), PermitError> {
        if self.granted.iter().any(|done| done == action_id) {
            return Err(PermitError::Denied(format!(
                "{action_id} was already granted"
            )));
        }
        let seq = self.next_seq;
        let reply = self
            .call(json!({"op": "permit", "action_id": action_id, "seq": seq}))
            .await?;
        if reply["granted"].as_bool() != Some(true) {
            let reason = reply["reason"].as_str().unwrap_or("denied");
            return Err(PermitError::Denied(reason.to_owned()));
        }
        if reply["action_id"] != action_id || reply["seq"].as_u64() != Some(seq) {
            return Err(PermitError::Denied(
                "reply does not match the request".into(),
            ));
        }
        Ok(())
    }

    /// The permit comes first. Nothing is dispatched on a refusal.
    async fn guarded<T>(
        &mut self,
        action_id: &str,
        dispatch: impl std::future::Future<Output = T>,
    ) -> Result<T, PermitError> {
        self.permit(action_id).await?;
        Ok(dispatch.await)
    }
}

// The five scenarios: explicit action lists, not the long-schedule generator.

const SETUP_S: u64 = 120;
const OBSERVATION_S: u64 = 180;
const ENDING_S: u64 = 300;
const CLEANUP_S: u64 = 60;
/// Seconds a transient leave must return within, before the absence grace.
const RETURN_WITHIN_S: u64 = 30;

#[derive(Clone, Copy, Debug, PartialEq)]
enum EndMode {
    Manual,
    ServerDeadline,
    Absence,
    /// The server closes the room (deleted by this run's own `room_delete`
    /// action) while the recovery write is held, so the agent's SDK reports
    /// `Disconnected` and the loop takes `Terminal::RoomClosed`.
    RoomClosed,
}

impl EndMode {
    fn manifest_name(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::ServerDeadline => "server_deadline",
            Self::Absence => "absence",
            Self::RoomClosed => "room_closed",
        }
    }
}

struct Scenario {
    name: &'static str,
    /// The nonfinal action IDs, in the order they are dispatched. The reserved
    /// `end` permit is not listed here, as in the manifest.
    actions: &'static [&'static str],
    /// Whether the script itself ends with the reserved `end`. An automatic
    /// terminal spends it only if cleanup needs it.
    scripted_end: bool,
    /// Hard ceiling on permits, the reserved End included.
    ceiling: usize,
    end_mode: EndMode,
    outer_cap_s: u64,
    report: bool,
}

const SCENARIOS: [Scenario; 5] = [
    Scenario {
        name: "a_manual_end_with_transient_leave",
        actions: &[
            "candidate_join",
            "seed_fixture",
            "controlled_recovery_trigger",
            "candidate_leave",
            "candidate_rejoin",
        ],
        scripted_end: true,
        ceiling: 6,
        end_mode: EndMode::Manual,
        outer_cap_s: 15 * 60,
        report: true,
    },
    Scenario {
        name: "b_server_deadline",
        actions: &[
            "candidate_join",
            "seed_fixture",
            "controlled_recovery_trigger",
        ],
        scripted_end: false,
        ceiling: 4,
        end_mode: EndMode::ServerDeadline,
        outer_cap_s: 20 * 60,
        report: true,
    },
    Scenario {
        name: "c_absence_expiry",
        actions: &[
            "candidate_join",
            "seed_fixture",
            "controlled_recovery_trigger",
            "candidate_leave",
        ],
        scripted_end: false,
        ceiling: 5,
        end_mode: EndMode::Absence,
        outer_cap_s: 15 * 60,
        report: false,
    },
    Scenario {
        name: "d_end_during_pending_recovery_with_thinking_hold",
        actions: &[
            "candidate_join",
            "seed_fixture",
            "thinking_hold",
            "controlled_recovery_trigger",
            "thinking_release",
        ],
        scripted_end: true,
        ceiling: 6,
        end_mode: EndMode::Manual,
        outer_cap_s: 15 * 60,
        report: true,
    },
    // The fifth case is the room being closed by the server while the recovery
    // write is held. Its End permit is reserved and, as in (b) and (c), the
    // automatic terminal does not spend it.
    Scenario {
        name: "e_room_closed_during_recovery",
        actions: &[
            "candidate_join",
            "seed_fixture",
            "controlled_recovery_trigger",
            "room_delete",
        ],
        scripted_end: false,
        ceiling: 5,
        end_mode: EndMode::RoomClosed,
        outer_cap_s: 15 * 60,
        report: false,
    },
];

/// Every interview in the five cases is the shortest the product admits.
const SCENARIO_DURATION_MIN: u32 = 10;

impl Scenario {
    /// The permits the script takes, in order.
    fn script(&self) -> Vec<&'static str> {
        let mut script = self.actions.to_vec();
        if self.scripted_end {
            script.push(FINAL_ACTION_ID);
        }
        script
    }

    fn observation_s(&self) -> u64 {
        match self.end_mode {
            EndMode::ServerDeadline => {
                super::super::super::interview_hard_deadline(SCENARIO_DURATION_MIN).as_secs()
            }
            EndMode::Manual | EndMode::Absence | EndMode::RoomClosed => OBSERVATION_S,
        }
    }

    fn selected_duration_s(&self) -> u64 {
        SETUP_S + self.observation_s()
    }

    fn cleanup_reserve_s(&self) -> u64 {
        ENDING_S + CLEANUP_S
    }

    /// The smallest `max_archive_bytes` the controller accepts for this cap:
    /// the sampler's worst case plus its own files, as `archive_bounds` there.
    fn archive_floor_bytes(&self) -> u64 {
        (self.outer_cap_s / 30 + 2) * 4096 + 256 * 1024
    }

    fn manifest(&self, candidate: &Path, candidate_sha256: &str) -> Value {
        json!({
            "schema_version": 1,
            "run_id": self.name.replace('_', "-"),
            "binaries": {"candidate": {
                "path": candidate.to_str().unwrap(),
                "sha256": candidate_sha256,
            }},
            "settings_sha256": "a".repeat(64),
            "fixture_sha256": "b".repeat(64),
            "selected_duration_s": self.selected_duration_s(),
            "outer_wall_cap_s": self.outer_cap_s,
            "cleanup_reserve_s": self.cleanup_reserve_s(),
            "end_mode": self.end_mode.manifest_name(),
            "run_kind": "smoke",
            "duration_min": SCENARIO_DURATION_MIN,
            "observation_target_s": self.observation_s(),
            "max_archive_bytes": self.archive_floor_bytes(),
            "actions": self.actions,
            "action_ceiling": self.actions.len() + 1,
            "limits": {},
            "scopes": {"scripted_actions": "hard", "wall_time": "hard"},
        })
    }
}

/// The interview length the run asks the product for. It follows the rules the
/// token endpoint applies: a whole number of minutes in the product's
/// range, not above what the recording cap would clamp a token to. A
/// server-deadline run must also observe exactly the server's own deadline,
/// taken from `interview_hard_deadline`, not restated here.
fn duration_min_from_manifest(
    manifest: &Value,
    recording_max_min: Option<u32>,
) -> Result<u32, String> {
    use crate::config::{MAX_DURATION_MIN, MIN_DURATION_MIN};
    let duration = manifest["duration_min"]
        .as_u64()
        .and_then(|minutes| u32::try_from(minutes).ok())
        .ok_or("duration_min must be a whole number of minutes")?;
    if !(MIN_DURATION_MIN..=MAX_DURATION_MIN).contains(&duration) {
        return Err(format!(
            "duration_min {duration} is outside the product range {MIN_DURATION_MIN}..{MAX_DURATION_MIN}"
        ));
    }
    let ceiling = crate::web::duration_ceiling(recording_max_min);
    if duration > ceiling {
        return Err(format!(
            "the recording cap would clamp the token to {ceiling} minutes, below the {duration} requested"
        ));
    }
    let target = manifest["observation_target_s"]
        .as_u64()
        .ok_or("observation_target_s must be a whole number of seconds")?;
    let server = super::super::super::interview_hard_deadline(duration).as_secs();
    match manifest["end_mode"].as_str() {
        Some("server_deadline") if target != server => Err(format!(
            "a server-deadline run observes the {server}s deadline, not {target}s"
        )),
        Some("server_deadline") => Ok(duration),
        Some("manual" | "absence" | "room_closed") if target > u64::from(duration) * 60 => Err(
            format!("observation_target_s {target} is longer than the {duration} minute interview"),
        ),
        Some("manual" | "absence" | "room_closed") => Ok(duration),
        other => Err(format!("end_mode {other:?} is not one of this fixture's")),
    }
}

// Running a script: the permit comes before every side effect.

/// The side effects a script performs. A read-only barrier is not an action
/// and takes no permit.
trait Effects {
    /// Whether every action of the live script has a real implementation. A
    /// live run checks this before its first permit, so an unwired fixture
    /// cannot spend a budgeted action on nothing.
    fn wired(&self) -> bool {
        true
    }
    async fn perform(&mut self, action: &str) -> Result<(), String>;
    /// Returns once the prepared-write gate of the recovery trigger has really
    /// been entered, not once the trigger was sent.
    async fn await_gate_entered(&mut self) -> Result<(), String>;
    /// Returns once the scenario's expected terminal has been observed, while
    /// the gate is still held.
    async fn await_terminal(&mut self, scenario: &Scenario) -> Result<(), String>;
    /// Retires the gate. Called only after the expected terminal.
    async fn release_gate(&mut self);
}

#[derive(Debug, PartialEq)]
enum ScriptError {
    Permit(String),
    Effect(String),
    /// The launcher asked the worker to stop. No new action starts; the
    /// caller ends with `cleanup_end`.
    Stopped,
}

/// Runs one scenario. A refused permit stops the script before its effect, so
/// the effects performed are always a prefix of the permits granted. The
/// launcher's stop request is read before every action but the reserved End.
async fn run_script(
    permits: &mut PermitClient,
    scenario: &Scenario,
    effects: &mut impl Effects,
) -> Result<(), ScriptError> {
    for action in scenario.script() {
        if action != FINAL_ACTION_ID && permits.stop_requested() {
            return Err(ScriptError::Stopped);
        }
        permits
            .guarded(action, effects.perform(action))
            .await
            .map_err(|error| ScriptError::Permit(error.to_string()))?
            .map_err(ScriptError::Effect)?;
        if action == "controlled_recovery_trigger" {
            effects
                .await_gate_entered()
                .await
                .map_err(ScriptError::Effect)?;
        }
    }
    effects
        .await_terminal(scenario)
        .await
        .map_err(ScriptError::Effect)?;
    effects.release_gate().await;
    Ok(())
}

/// The reserved End, for a cleanup that needs one after an automatic terminal
/// or a failed script. It is the same permit the scripted End spends, so a
/// run that already ended cannot spend a second.
async fn cleanup_end(
    permits: &mut PermitClient,
    effects: &mut impl Effects,
) -> Result<(), ScriptError> {
    permits
        .guarded(FINAL_ACTION_ID, effects.perform(FINAL_ACTION_ID))
        .await
        .map_err(|error| ScriptError::Permit(error.to_string()))?
        .map_err(ScriptError::Effect)
}

/// Ends a run that a script left open: it failed or was stopped after the
/// candidate joined and has not spent End. A script that completed reached its
/// expected terminal, and a run that never joined has no room to end.
async fn cleanup_end_if_needed(
    permits: &mut PermitClient,
    effects: &mut impl Effects,
    result: &Result<(), ScriptError>,
) -> Result<(), ScriptError> {
    let spent = |id: &str| permits.granted.iter().any(|done| done == id);
    if result.is_ok() || spent(FINAL_ACTION_ID) || !spent("candidate_join") {
        return Ok(());
    }
    cleanup_end(permits, effects).await
}

// The prepared-write gate on the real room loop's recovery write.

/// The recovery write the trigger holds. `recovery_brief` is the same phase
/// the recovery matrices hold on a hand-built loop.
const RECOVERY_GATE_PHASE: &str = "recovery_brief";

struct RecoveryGate {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl RecoveryGate {
    /// Arms the gate for the next room loop that starts in this process.
    fn arm() -> Self {
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        super::super::super::LIVE_WRITE_BARRIERS
            .lock()
            .unwrap()
            .push_back(super::super::WriteBarrier {
                phase: RECOVERY_GATE_PHASE,
                entered: std::sync::Arc::clone(&entered),
                release: std::sync::Arc::clone(&release),
            });
        Self { entered, release }
    }

    async fn entered(&self, within: Duration) -> Result<(), String> {
        tokio::time::timeout(within, self.entered.notified())
            .await
            .map_err(|_| "the prepared write was not reached".to_owned())
    }

    fn release(&self) {
        self.release.notify_one();
    }
}

// The candidate's report receipts, counted.

/// What the candidate has seen on the report topic and acknowledged.
#[derive(Default)]
struct ReportTally {
    final_ids: std::collections::BTreeSet<String>,
    /// Final reports received, copies of one report included: a resend of the
    /// same bytes is a second receipt even though it is not a second report.
    final_copies: usize,
    provisional: usize,
    receipts: usize,
}

impl ReportTally {
    /// Decodes one received packet as the page would. Returns the receipt to
    /// publish, or `None` for anything that is not a report.
    fn observe(&mut self, topic: Option<&str>, payload: &[u8]) -> Option<DataPacket> {
        let ack = super::super::super::report::candidate_ack(topic, payload)?;
        if super::super::super::report::is_final_report(payload) {
            self.final_ids.insert(crate::sha256_hex(&[payload]));
            self.final_copies += 1;
        } else {
            self.provisional += 1;
        }
        self.receipts += 1;
        Some(ack)
    }

    /// Report-producing cases need exactly one final report; the others none.
    fn matches(&self, scenario: &Scenario) -> bool {
        self.final_ids.len() == usize::from(scenario.report)
    }

    /// The live verdict: exactly one final report receipt in a report case,
    /// none otherwise, counting a second copy of the same report.
    fn exactly(&self, scenario: &Scenario) -> bool {
        self.matches(scenario) && self.final_copies == usize::from(scenario.report)
    }
}

// The live test. NOT RUN.

/// Every effect the live script needs is a real SDK or provider action, and
/// none is wired. A script that reaches this stops with an error.
struct UnwiredEffects;

impl Effects for UnwiredEffects {
    fn wired(&self) -> bool {
        false
    }
    async fn perform(&mut self, action: &str) -> Result<(), String> {
        Err(format!("NOT RUN: no SDK effect is wired for {action}"))
    }
    async fn await_gate_entered(&mut self) -> Result<(), String> {
        Err("NOT RUN: no room loop is hosted to enter the gate".into())
    }
    async fn await_terminal(&mut self, _: &Scenario) -> Result<(), String> {
        Err("NOT RUN: no terminal can be observed".into())
    }
    async fn release_gate(&mut self) {}
}

// What the candidate's room shows, as a state machine the live observers feed
// and the offline tests drive.

/// One thing the candidate's side of the room saw or did.
#[derive(Clone, Debug, PartialEq)]
enum Sighting {
    /// This fixture closed the candidate's connection.
    CandidateLeft,
    /// This fixture reconnected the candidate.
    CandidateReturned,
    /// The candidate's connection reported the close this fixture asked for.
    /// It is the candidate's own departure seen from its side, not an ending.
    OwnClose,
    /// The interviewer left the room.
    AgentLeft,
    /// The room-hosting task in this process returned.
    AgentTaskFinished,
    /// The server disconnected the candidate, with LiveKit's stated reason.
    RoomClosed(String),
    /// A control packet from the interviewer, by its `type`.
    Control(String),
}

/// Whether the interview is over, from what was seen, not from what was done.
/// The candidate leaving, however briefly, is a sighting of the candidate and
/// never an ending: only the interviewer going, its task finishing or the
/// server closing the room is.
#[derive(Default)]
struct PresenceObserver {
    log: Vec<(Duration, Sighting)>,
    candidate_absent: bool,
    transient_leaves: usize,
    agent_left_at: Option<Duration>,
    finished_at: Option<Duration>,
    closed_at: Option<Duration>,
}

impl PresenceObserver {
    fn note(&mut self, at: Duration, sighting: Sighting) {
        match &sighting {
            Sighting::CandidateLeft => self.candidate_absent = true,
            Sighting::CandidateReturned => {
                if self.candidate_absent {
                    self.transient_leaves += 1;
                }
                self.candidate_absent = false;
            }
            Sighting::AgentLeft => {
                self.agent_left_at.get_or_insert(at);
            }
            Sighting::AgentTaskFinished => {
                self.finished_at.get_or_insert(at);
            }
            Sighting::RoomClosed(_) => {
                self.closed_at.get_or_insert(at);
            }
            Sighting::OwnClose | Sighting::Control(_) => {}
        }
        self.log.push((at, sighting));
    }

    fn ended(&self) -> bool {
        self.agent_left_at.is_some() || self.finished_at.is_some() || self.closed_at.is_some()
    }

    /// When it ended, as the earliest ending sighting.
    fn ended_at(&self) -> Option<Duration> {
        [self.agent_left_at, self.finished_at, self.closed_at]
            .into_iter()
            .flatten()
            .min()
    }
}

/// What a run saw, reduced to what the verdict reads.
struct Facts {
    gate_entered: bool,
    agent: AgentEnd,
    final_copies: usize,
    final_distinct: usize,
    provisional: usize,
    ack_failures: usize,
    /// The interview ended before the script's own ending was sent, in a case
    /// whose script sends one.
    ended_before_end: bool,
    /// The interviewer was still in the room when the candidate came back.
    agent_after_return: Option<bool>,
    /// A scripted End was sent while the interviewer's task was still running
    /// and the recovery write was still held.
    end_while_pending: Option<bool>,
    /// Seconds from the interview being ready to the interviewer's task
    /// finishing.
    deadline_elapsed_s: Option<f64>,
    /// Seconds from the candidate leaving to the interviewer's task finishing.
    absence_elapsed_s: Option<f64>,
    /// Identities in the room before this run deleted it. `None` when the
    /// room was already gone.
    participants_left: Option<Vec<String>>,
    readers_unfinished: usize,
    room_gone_after_delete: bool,
    /// The recovery terminal the room loop took, by name, as the loop reported
    /// it. `None` when the loop reported none.
    terminal: Option<String>,
    /// Whether the run's own `room_delete` was issued only after the
    /// recovery write was held. `None` when the run issued no delete.
    delete_after_gate: Option<bool>,
    /// Whether the terminal was reported no earlier than the delete was
    /// issued. `None` when either was not observed.
    terminal_after_delete: Option<bool>,
}

/// How the hosted room loop returned.
#[derive(Clone, Debug, PartialEq)]
enum AgentEnd {
    Completed,
    NoCandidate,
    Failed(String),
    /// Still running when the fixture gave up on it and aborted it.
    Aborted,
}

/// The failures a run's facts show. An empty list is a pass.
fn judge(scenario: &Scenario, facts: &Facts) -> Vec<String> {
    let mut problems = Vec::new();
    if !facts.gate_entered {
        problems.push("the prepared recovery write was never entered".to_owned());
    }
    if facts.agent != AgentEnd::Completed {
        problems.push(format!("the room loop did not complete: {:?}", facts.agent));
    }
    let wanted = usize::from(scenario.report);
    if facts.final_copies != wanted || facts.final_distinct != wanted {
        problems.push(format!(
            "expected exactly {wanted} final report receipt, saw {} copies of {} distinct",
            facts.final_copies, facts.final_distinct
        ));
    }
    if facts.ack_failures != 0 {
        problems.push(format!(
            "{} acknowledgements failed to send",
            facts.ack_failures
        ));
    }
    if facts.ended_before_end {
        problems.push("the interview ended before the scripted End".to_owned());
    }
    match scenario.end_mode {
        EndMode::Manual => {
            if scenario.name.starts_with('a') && facts.agent_after_return != Some(true) {
                problems.push("the interviewer was not in the room after the return".into());
            }
            if facts.end_while_pending != Some(true) {
                problems.push("End was not sent while the recovery was still pending".into());
            }
        }
        EndMode::ServerDeadline => {
            let deadline =
                super::super::super::interview_hard_deadline(SCENARIO_DURATION_MIN).as_secs_f64();
            match facts.deadline_elapsed_s {
                Some(elapsed)
                    if elapsed + DEADLINE_SLACK_S >= deadline
                        && elapsed <= deadline + ENDING_S as f64 => {}
                other => problems.push(format!(
                    "the deadline ended it at {other:?}s, not at the {deadline}s hard deadline"
                )),
            }
        }
        EndMode::RoomClosed => {
            match facts.terminal.as_deref() {
                Some("RoomClosed") => {}
                other => problems.push(format!(
                    "the terminal was {other:?}, not RoomClosed from the server closing the room"
                )),
            }
            if facts.delete_after_gate != Some(true) {
                problems.push("the room was not deleted after the recovery write was held".into());
            }
            if facts.terminal_after_delete != Some(true) {
                problems.push("the terminal was not observed after the room was deleted".into());
            }
        }
        EndMode::Absence => {
            let grace = super::super::super::CANDIDATE_ABSENCE_LIMIT.as_secs_f64();
            match facts.absence_elapsed_s {
                Some(elapsed) if elapsed + 1.0 >= grace => {}
                other => problems.push(format!(
                    "absence ended it at {other:?}s, before the {grace}s grace"
                )),
            }
        }
    }
    if let Some(left) = &facts.participants_left
        && !left.is_empty()
    {
        problems.push(format!("participants left in the room: {left:?}"));
    }
    if !facts.room_gone_after_delete {
        problems.push("the room was still listed after it was deleted".into());
    }
    if facts.readers_unfinished != 0 {
        problems.push(format!(
            "{} Live reader tasks were still running",
            facts.readers_unfinished
        ));
    }
    problems
}

/// Seconds a deadline observation may fall short of the hard deadline: the
/// probe that marks the interview ready is sent just after the loop's deadline
/// is computed from its own clock, and the task is observed finishing on a
/// poll.
const DEADLINE_SLACK_S: f64 = 5.0;

/// Secrets of this run that must never reach a log, whatever text carries
/// them. Registered once the config is loaded.
static SECRETS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

fn register_secrets(values: impl IntoIterator<Item = String>) {
    SECRETS
        .lock()
        .unwrap()
        .extend(values.into_iter().filter(|value| value.len() >= 6));
}

/// Removes what must not reach a log from an error's text: a credential
/// carried in a URL query or an authorization header, any case, and the run's
/// own registered secrets wherever they appear.
fn redact(text: &str) -> String {
    const NAMES: [&str; 7] = [
        "access_token=",
        "api_key=",
        "key=",
        "token=",
        "bearer ",
        "key\": \"",
        "key\":\"",
    ];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some((start, name)) = NAMES
            .iter()
            .filter_map(|name| lower.find(name).map(|at| (at, name.len())))
            .min()
        else {
            break;
        };
        out.push_str(&rest[..start + name]);
        out.push_str("REDACTED");
        let value = &rest[start + name..];
        let end = value
            .find(['&', ' ', '"', '\'', ')', '\n', '>'])
            .unwrap_or(value.len());
        rest = &value[end..];
    }
    out.push_str(rest);
    for secret in SECRETS.lock().unwrap().iter() {
        out = out.replace(secret.as_str(), "REDACTED");
    }
    out
}

// The live wiring: a real SDK candidate in a real room, the real room loop in
// this process, and the observers between them.

use ::livekit::prelude::{Room, RoomEvent, RoomOptions};

/// What the observers share with the effects.
struct Observed {
    tally: ReportTally,
    presence: PresenceObserver,
    ack_failures: usize,
}

struct Shared {
    epoch: std::time::Instant,
    observed: std::sync::Mutex<Observed>,
}

impl Shared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            epoch: std::time::Instant::now(),
            observed: std::sync::Mutex::new(Observed {
                tally: ReportTally::default(),
                presence: PresenceObserver::default(),
                ack_failures: 0,
            }),
        })
    }

    fn note(&self, sighting: Sighting) {
        let at = self.epoch.elapsed();
        emit(
            "sighting",
            json!({"at_ms": at.as_millis() as u64, "sighting": format!("{sighting:?}")}),
        );
        self.observed.lock().unwrap().presence.note(at, sighting);
    }
}

/// One line of the archive. It carries names, counts and times and nothing
/// that came from a credential.
fn emit(kind: &str, fields: Value) {
    eprintln!("LIVE_OBS {}", json!({"kind": kind, "fields": fields}));
}

/// Reads the candidate's room events: acknowledges the report as the page
/// does, counts what arrives, and notes who left.
fn spawn_observer(
    shared: Arc<Shared>,
    mut events: tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
    me: ::livekit::prelude::LocalParticipant,
    agent_identity: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                RoomEvent::DataReceived {
                    payload,
                    topic,
                    participant,
                    ..
                } => {
                    let sender = participant.as_ref().map(|p| p.identity().0);
                    if sender.as_deref().is_some_and(|who| who != agent_identity) {
                        continue;
                    }
                    if topic.as_deref() == Some(crate::runtime::TOPIC_CONTROL) {
                        if let Ok(value) = serde_json::from_slice::<Value>(&payload)
                            && let Some(kind) = value["type"].as_str()
                        {
                            shared.note(Sighting::Control(kind.to_owned()));
                        }
                        continue;
                    }
                    let ack = shared
                        .observed
                        .lock()
                        .unwrap()
                        .tally
                        .observe(topic.as_deref(), &payload);
                    if let Some(ack) = ack {
                        emit(
                            "report_packet",
                            json!({
                                "final": super::super::super::report::is_final_report(&payload),
                                "bytes": payload.len(),
                            }),
                        );
                        if me.publish_data(ack).await.is_err() {
                            shared.observed.lock().unwrap().ack_failures += 1;
                        }
                    }
                }
                RoomEvent::ParticipantDisconnected(who) if who.identity().0 == agent_identity => {
                    shared.note(Sighting::AgentLeft);
                }
                RoomEvent::Disconnected { reason } => {
                    shared.note(if reason == ::livekit::DisconnectReason::ClientInitiated {
                        Sighting::OwnClose
                    } else {
                        Sighting::RoomClosed(format!("{reason:?}"))
                    });
                }
                _ => {}
            }
        }
    })
}

struct CandidateLink {
    room: Room,
    observer: tokio::task::JoinHandle<()>,
}

async fn connect_candidate(
    url: &str,
    token: &str,
    shared: &Arc<Shared>,
    agent_identity: &str,
) -> Result<CandidateLink, String> {
    crate::install_crypto_provider();

    // Nothing the interviewer publishes is played: the fixture reads data
    // packets, so it takes no media.
    let mut options = RoomOptions::default();
    options.auto_subscribe = false;
    let (room, events) = tokio::time::timeout(CONNECT_LIMIT, Room::connect(url, token, options))
        .await
        .map_err(|_| "candidate connect timed out".to_owned())?
        .map_err(|error| format!("candidate connect failed: {}", redact(&error.to_string())))?;
    let observer = spawn_observer(
        std::sync::Arc::clone(shared),
        events,
        room.local_participant(),
        agent_identity.to_owned(),
    );
    Ok(CandidateLink { room, observer })
}

/// How long the candidate's SDK connect may take.
const CONNECT_LIMIT: Duration = Duration::from_secs(30);
/// Seconds the transient leave stays out, well inside the absence grace.
const TRANSIENT_ABSENCE: Duration = Duration::from_secs(10);
/// How long the prepared recovery write may take to be reached after the
/// trigger.
const GATE_WAIT: Duration = Duration::from_secs(90);
/// How long the interviewer may take to appear in the room after a return.
const RETURN_VISIBILITY: Duration = Duration::from_secs(5);

/// The synthetic editor state seeded as report input. It is authored text sent
/// on the code topic and does not certify that speech was captured.
const SEED_STARTER: &str = "def two_sum(nums, target):\n    pass\n";
const SEED_SOLUTION: &str = "def two_sum(nums, target):\n    seen = {}\n    for i, n in enumerate(nums):\n        if target - n in seen:\n            return [seen[target - n], i]\n        seen[n] = i\n    return []\n";

struct LiveEffects<'a> {
    scenario: &'static Scenario,
    config: Arc<crate::config::AgentConfig>,
    gate: &'a RecoveryGate,
    stop_file: Option<PathBuf>,
    room_name: String,
    candidate_identity: String,
    agent_identity: String,
    token: String,
    shared: Arc<Shared>,
    link: Option<CandidateLink>,
    agent: Option<tokio::task::JoinHandle<AgentEnd>>,
    agent_finished: Arc<std::sync::OnceLock<std::time::Instant>>,
    agent_end: Option<AgentEnd>,
    readers: tokio::sync::mpsc::UnboundedReceiver<crate::gemini::LiveReaderInspection>,
    first_reader: Option<crate::gemini::LiveReaderInspection>,
    kept_readers: Vec<crate::gemini::LiveReaderInspection>,
    ready_at: Option<std::time::Instant>,
    left_at: Option<std::time::Instant>,
    end_sent: bool,
    /// Set once the `candidate_join` permit let the agent task start. The
    /// room is cleaned up only if this run could have created it.
    joined: bool,
    /// Seconds the interview may take to start after the join permit.
    setup_s: u64,
    /// The scripted waits stop here: the cap less the cleanup minute.
    script_stop: std::time::Instant,
    /// Nothing runs past here: the outer wall cap.
    hard_stop: std::time::Instant,
    tearing_down: bool,
    gate_entered: bool,
    ended_before_end: bool,
    agent_after_return: Option<bool>,
    end_while_pending: Option<bool>,
    /// The recovery terminals the room loop reported, with when.
    terminals: tokio::sync::mpsc::UnboundedReceiver<(&'static str, std::time::Instant)>,
    terminal: Option<(&'static str, std::time::Instant)>,
    /// When this run issued its own `room_delete`, and whether the recovery
    /// write was held at that moment.
    deleted_at: Option<std::time::Instant>,
    delete_after_gate: Option<bool>,
}

include!("recovery_live_parts/launcher_and_scenario_judging.rs");
include!("recovery_live_parts/native_worker_and_room_disconnect.rs");

// The speech-continuity arms: a stalled microphone consumer against the real
// room loop, one clip arm per invocation.
#[path = "recovery_continuity.rs"]
mod continuity;

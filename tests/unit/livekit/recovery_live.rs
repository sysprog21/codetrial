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

// The four scenarios: explicit action lists, not the long-schedule generator.

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
}

impl EndMode {
    fn manifest_name(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::ServerDeadline => "server_deadline",
            Self::Absence => "absence",
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

const SCENARIOS: [Scenario; 4] = [
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
];

/// Every interview in the four cases is the shortest the product admits.
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
            EndMode::Manual | EndMode::Absence => OBSERVATION_S,
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
/// token endpoint applies in T8a.1: a whole number of minutes in the product's
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
        Some("manual" | "absence") if target > u64::from(duration) * 60 => Err(format!(
            "observation_target_s {target} is longer than the {duration} minute interview"
        )),
        Some("manual" | "absence") => Ok(duration),
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
                entered: entered.clone(),
                release: release.clone(),
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
        shared.clone(),
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
    /// The scripted waits stop here: the cap less the cleanup minute.
    script_stop: std::time::Instant,
    /// Nothing runs past here: the outer wall cap.
    hard_stop: std::time::Instant,
    tearing_down: bool,
    gate_entered: bool,
    ended_before_end: bool,
    agent_after_return: Option<bool>,
    end_while_pending: Option<bool>,
}

impl<'a> LiveEffects<'a> {
    /// Everything that can be built without touching a provider: the candidate
    /// join token is signed here, locally, and not used until the permitted
    /// join.
    fn new(
        scenario: &'static Scenario,
        config: crate::config::AgentConfig,
        recording_cap: Option<u32>,
        duration: u32,
        gate: &'a RecoveryGate,
        stop_file: Option<PathBuf>,
    ) -> Result<Self, String> {
        let letter = &scenario.name[..1];
        let stamp = crate::current_epoch_millis();
        let room_name = format!("ct-live-{letter}-{stamp}");
        let candidate_identity = format!("candidate-live{letter}");
        let body = json!({"problemId": "two-sum", "durationMin": duration}).to_string();
        let minted = crate::web::token_response(
            &crate::web::TokenConfig {
                api_key: &config.livekit_api_key,
                api_secret: &config.livekit_api_secret,
                server_url: &config.livekit_url,
                recording_max_min: recording_cap,
            },
            body.as_bytes(),
            &room_name,
            &candidate_identity,
            crate::current_epoch_seconds(),
        )
        .map_err(|error| format!("cannot mint the candidate token: {error}"))?;
        if minted.duration_min != json!(duration) {
            return Err(format!(
                "the token endpoint's rules gave {} minutes, not {duration}",
                minted.duration_min
            ));
        }
        let (probe, readers) = tokio::sync::mpsc::unbounded_channel();
        *super::super::super::LIVE_READER_PROBE.lock().unwrap() = Some(probe);
        Ok(Self {
            scenario,
            agent_identity: crate::runtime::agent_identity(&room_name),
            config: Arc::new(config),
            gate,
            stop_file,
            room_name,
            candidate_identity,
            token: minted.token.clone(),
            shared: Shared::new(),
            link: None,
            agent: None,
            agent_finished: Arc::new(std::sync::OnceLock::new()),
            agent_end: None,
            readers,
            first_reader: None,
            kept_readers: Vec::new(),
            ready_at: None,
            left_at: None,
            end_sent: false,
            joined: false,
            script_stop: std::time::Instant::now() + script_budget(scenario),
            hard_stop: std::time::Instant::now() + Duration::from_secs(scenario.outer_cap_s),
            tearing_down: false,
            gate_entered: false,
            ended_before_end: false,
            agent_after_return: None,
            end_while_pending: None,
        })
    }

    fn stop_requested(&self) -> bool {
        self.stop_file.as_deref().is_some_and(Path::exists)
    }

    fn agent_running(&self) -> bool {
        self.agent.is_some() && self.agent_finished.get().is_none()
    }

    /// Whether the interview is over, by the task and by what the candidate's
    /// side of the room saw.
    fn interview_over(&self) -> bool {
        !self.agent_running() || self.shared.observed.lock().unwrap().presence.ended()
    }

    /// Polls `done` until it holds, the time runs out, the launcher asks for a
    /// stop or the interviewer's task finishes when `until_agent_done` is not
    /// what is being waited for.
    async fn wait_for(
        &self,
        within: Duration,
        what: &str,
        mut done: impl FnMut(&Self) -> bool,
    ) -> Result<(), String> {
        let until = tokio::time::Instant::now() + within;
        loop {
            if done(self) {
                return Ok(());
            }
            if self.stop_requested() {
                return Err(format!("stopped by the launcher while waiting for {what}"));
            }
            if !self.tearing_down && std::time::Instant::now() >= self.script_stop {
                return Err(format!("the outer wall cap came while waiting for {what}"));
            }
            if tokio::time::Instant::now() >= until {
                return Err(format!("timed out after {within:?} waiting for {what}"));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    fn drain_readers(&mut self) {
        while let Ok(reader) = self.readers.try_recv() {
            if self.first_reader.is_none() {
                self.ready_at = Some(std::time::Instant::now());
                self.first_reader = Some(reader);
            } else {
                self.kept_readers.push(reader);
            }
        }
    }

    // LiveKit closes a room that only agents remain in once its departure
    // timeout passes (the server default is 20 s), long before the interview's
    // 90 s absence grace: the first live run of this scenario saw `RoomClosed`
    // about 28 s after the candidate left. Production now creates its rooms
    // explicitly in `/api/token`; this scenario uses that same call, so it
    // exercises the departure timeout production actually sends.
    async fn create_room_for_absence(&self) -> Result<(), String> {
        super::super::super::room_service::create_interview_room(
            &self.config.livekit_url,
            &self.config.livekit_api_key,
            &self.config.livekit_api_secret,
            &self.room_name,
            crate::current_epoch_seconds(),
        )
        .await
        .map_err(|error| redact(&error.to_string()))
    }

    async fn join(&mut self) -> Result<(), String> {
        let began = std::time::Instant::now();
        self.joined = true;
        if matches!(self.scenario.end_mode, EndMode::Absence) {
            self.create_room_for_absence().await?;
        }
        let config = self.config.clone();
        let room = self.room_name.clone();
        let finished = self.agent_finished.clone();
        let shared = self.shared.clone();
        self.agent = Some(tokio::spawn(async move {
            let outcome = super::super::super::run_room_with_slot(
                &config,
                &room,
                crate::current_epoch_seconds(),
                None,
            )
            .await;
            let _ = finished.set(std::time::Instant::now());
            shared.note(Sighting::AgentTaskFinished);
            match outcome {
                Ok(super::super::super::RoomRunOutcome::Completed) => AgentEnd::Completed,
                Ok(super::super::super::RoomRunOutcome::NoCandidate) => AgentEnd::NoCandidate,
                Ok(super::super::super::RoomRunOutcome::FailedBeforeCandidate(error))
                | Err(error) => AgentEnd::Failed(redact(&error.to_string())),
            }
        }));
        self.link = Some(
            connect_candidate(
                &self.config.livekit_url,
                &self.token,
                &self.shared,
                &self.agent_identity,
            )
            .await?,
        );
        emit(
            "candidate_joined",
            json!({"room": self.room_name, "identity": self.candidate_identity}),
        );
        self.wait_for(
            Duration::from_secs(SETUP_S).saturating_sub(began.elapsed()),
            "the interview to start",
            |this| !this.agent_running() || this.first_reader_probed(),
        )
        .await
        .ok();
        self.drain_readers();
        if self.first_reader.is_none() {
            return Err(format!(
                "the interview did not start: agent running {}",
                self.agent_running()
            ));
        }
        emit(
            "interview_ready",
            json!({"after_ms": began.elapsed().as_millis() as u64}),
        );
        Ok(())
    }

    fn first_reader_probed(&self) -> bool {
        // `readers` is drained by `&mut self`; peeking needs the same, so the
        // waiter reads the flag the loop sets through the channel's length.
        !self.readers.is_empty()
    }

    async fn publish(&self, topic: &str, payload: &Value) -> Result<(), String> {
        let link = self.link.as_ref().ok_or("the candidate is not connected")?;
        let packet = super::super::super::browser_packet(topic, payload)
            .map_err(|error| error.to_string())?;
        link.room
            .local_participant()
            .publish_data(packet)
            .await
            .map_err(|error| redact(&error.to_string()))
    }

    async fn leave(&mut self) -> Result<(), String> {
        let link = self.link.take().ok_or("the candidate is not connected")?;
        self.left_at = Some(std::time::Instant::now());
        self.shared.note(Sighting::CandidateLeft);
        let closed = link.room.close().await;
        link.observer.abort();
        closed.map_err(|error| redact(&error.to_string()))?;
        if self.scenario.end_mode == EndMode::Manual {
            // A transient absence is held while the recovery is pending, and
            // the interview must still be running at the end of it.
            tokio::time::sleep(TRANSIENT_ABSENCE).await;
            if self.interview_over() {
                self.ended_before_end = true;
                return Err("the interview ended during a transient absence".into());
            }
        }
        Ok(())
    }

    async fn rejoin(&mut self) -> Result<(), String> {
        let left = self.left_at.ok_or("the candidate never left")?;
        if left.elapsed() >= Duration::from_secs(RETURN_WITHIN_S) {
            return Err("the return is not before the absence grace".into());
        }
        if self.interview_over() {
            self.ended_before_end = true;
            return Err("the interview ended before the candidate returned".into());
        }
        let link = connect_candidate(
            &self.config.livekit_url,
            &self.token,
            &self.shared,
            &self.agent_identity,
        )
        .await?;
        self.shared.note(Sighting::CandidateReturned);
        let seen = {
            let identity = self.agent_identity.clone();
            let began = std::time::Instant::now();
            loop {
                let present = link
                    .room
                    .remote_participants()
                    .keys()
                    .any(|who| who.0 == identity);
                if present || began.elapsed() >= RETURN_VISIBILITY {
                    break present;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        };
        self.agent_after_return = Some(seen);
        self.link = Some(link);
        Ok(())
    }

    async fn end(&mut self) -> Result<(), String> {
        let pending = !self.interview_over() && self.gate_entered && !self.end_sent;
        self.end_while_pending = Some(pending);
        if self.interview_over() && self.scenario.scripted_end {
            self.ended_before_end = true;
        }
        self.publish(
            crate::runtime::TOPIC_CONTROL,
            &json!({"type": "end_interview", "reason": "candidate_ended"}),
        )
        .await?;
        self.end_sent = true;
        Ok(())
    }

    async fn seed(&self) -> Result<(), String> {
        for code in [SEED_STARTER, SEED_SOLUTION] {
            self.publish(
                crate::runtime::TOPIC_CODE_UPDATE,
                &json!({"code": code, "language": "python"}),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok(())
    }

    /// Everything after the script: the candidate and the interviewer's task
    /// are retired, the room is checked for anyone left and then deleted, and
    /// every Live reader is checked finished.
    async fn teardown(&mut self) -> Teardown {
        let mut notes = Vec::new();
        self.tearing_down = true;
        if let Some(link) = self.link.take() {
            let _ = link.room.close().await;
            link.observer.abort();
        }
        if self.agent.is_some() && self.agent_end.is_none() {
            // An End the cleanup sent starts the ending window; anything else
            // only has the cleanup's own half minute before the abort.
            let patience = if self.end_sent {
                // What the cap leaves, keeping twenty seconds for the room
                // checks and the delete that must not be skipped.
                (self.hard_stop - Duration::from_secs(20))
                    .saturating_duration_since(std::time::Instant::now())
                    .min(Duration::from_secs(ENDING_S))
            } else {
                Duration::from_secs(CLEANUP_S / 2)
            };
            let finished = self
                .wait_for(patience, "the interviewer to finish", |this| {
                    this.agent_finished.get().is_some()
                })
                .await;
            if finished.is_err() {
                if let Some(handle) = &self.agent {
                    handle.abort();
                }
                notes.push("the interviewer's task was aborted".to_owned());
                self.agent_end = Some(AgentEnd::Aborted);
            }
        }
        if self.agent_end.is_none()
            && let Some(handle) = self.agent.take()
        {
            self.agent_end = Some(handle.await.unwrap_or(AgentEnd::Aborted));
        }
        self.drain_readers();
        if !self.joined {
            // The join permit was never spent: no room was created, so no
            // provider call is made to look for one.
            return Teardown {
                participants: None,
                room_gone_after_delete: true,
                readers_unfinished: 0,
                notes,
            };
        }
        let participants = self.settled_participants().await;
        let deleted = super::super::super::room_service::delete_room(
            &self.config.livekit_url,
            &self.config.livekit_api_key,
            &self.config.livekit_api_secret,
            &self.room_name,
            crate::current_epoch_seconds(),
        )
        .await;
        if let Err(error) = &deleted {
            notes.push(format!(
                "deleting the room failed: {}",
                redact(&error.to_string())
            ));
        }
        let gone = matches!(self.listed_participants().await, Ok(None));
        let mut unfinished = 0;
        let began = std::time::Instant::now();
        let readers: Vec<_> = self.first_reader.iter().chain(&self.kept_readers).collect();
        while readers.iter().any(|reader| !reader.is_finished())
            && began.elapsed() < Duration::from_secs(5)
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for reader in readers {
            if !reader.is_finished() {
                unfinished += 1;
            }
        }
        Teardown {
            participants: match participants {
                Ok(listed) => listed,
                Err(error) => {
                    notes.push(format!("listing the room failed: {error}"));
                    None
                }
            },
            room_gone_after_delete: gone,
            readers_unfinished: unfinished,
            notes,
        }
    }

    /// Identities in the room once LiveKit has had a few seconds to apply the
    /// last disconnect. `Ok(None)` when the room no longer exists.
    async fn settled_participants(&self) -> Result<Option<Vec<String>>, String> {
        let began = std::time::Instant::now();
        loop {
            let listed = self.listed_participants().await?;
            let empty = listed.as_ref().is_none_or(Vec::is_empty);
            if empty || began.elapsed() >= Duration::from_secs(10) {
                return Ok(listed);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn listed_participants(&self) -> Result<Option<Vec<String>>, String> {
        let token = crate::token::livekit_room_admin_token(
            &self.config.livekit_api_key,
            &self.config.livekit_api_secret,
            &self.room_name,
            crate::current_epoch_seconds(),
        )
        .map_err(|error| error.to_string())?;
        let (status, text) = super::super::super::room_service::post_room_service(
            &crate::config::livekit_http_base(&self.config.livekit_url),
            &token,
            "ListParticipants",
            &json!({"room": self.room_name}),
        )
        .await
        .map_err(|error| redact(&error.to_string()))?;
        if super::super::super::room_service::is_twirp_not_found(status, &text) {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(format!("ListParticipants answered {status}"));
        }
        let body: Value =
            serde_json::from_str(&text).map_err(|_| "ListParticipants was not JSON")?;
        Ok(Some(
            body["participants"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|who| who["identity"].as_str().map(str::to_owned))
                .collect(),
        ))
    }

    fn facts(&self, teardown: &Teardown) -> Facts {
        let observed = self.shared.observed.lock().unwrap();
        let ready = self.ready_at;
        let finished = self.agent_finished.get().copied();
        let elapsed = |from: Option<std::time::Instant>| match (from, finished) {
            (Some(from), Some(to)) => Some(to.saturating_duration_since(from).as_secs_f64()),
            _ => None,
        };
        Facts {
            gate_entered: self.gate_entered,
            agent: self.agent_end.clone().unwrap_or(AgentEnd::Aborted),
            final_copies: observed.tally.final_copies,
            final_distinct: observed.tally.final_ids.len(),
            provisional: observed.tally.provisional,
            ack_failures: observed.ack_failures,
            ended_before_end: self.ended_before_end,
            agent_after_return: self.agent_after_return,
            end_while_pending: self.end_while_pending,
            deadline_elapsed_s: elapsed(ready),
            absence_elapsed_s: elapsed(self.left_at),
            participants_left: teardown.participants.clone(),
            readers_unfinished: teardown.readers_unfinished,
            room_gone_after_delete: teardown.room_gone_after_delete,
        }
    }
}

struct Teardown {
    participants: Option<Vec<String>>,
    room_gone_after_delete: bool,
    readers_unfinished: usize,
    notes: Vec<String>,
}

impl Effects for LiveEffects<'_> {
    async fn perform(&mut self, action: &str) -> Result<(), String> {
        emit("action", json!({"action": action}));
        match action {
            "candidate_join" => self.join().await,
            "seed_fixture" => self.seed().await,
            "controlled_recovery_trigger" => {
                self.drain_readers();
                let reader = self.first_reader.as_ref().ok_or("no Live reader to end")?;
                reader.abort();
                Ok(())
            }
            "thinking_hold" => {
                self.publish(
                    crate::runtime::TOPIC_CONTROL,
                    &json!({"type": "thinking", "thinking": true}),
                )
                .await
            }
            "thinking_release" => {
                self.publish(
                    crate::runtime::TOPIC_CONTROL,
                    &json!({"type": "thinking", "thinking": false}),
                )
                .await
            }
            "candidate_leave" => self.leave().await,
            "candidate_rejoin" => self.rejoin().await,
            FINAL_ACTION_ID => self.end().await,
            other => Err(format!("{other} has no live effect")),
        }
    }

    async fn await_gate_entered(&mut self) -> Result<(), String> {
        let until = tokio::time::Instant::now() + GATE_WAIT;
        loop {
            if self.stop_requested() {
                return Err("stopped by the launcher before the gate was entered".into());
            }
            if !self.agent_running() {
                return Err("the interviewer ended before the gate was entered".into());
            }
            let slice = Duration::from_secs(1)
                .min(until.saturating_duration_since(tokio::time::Instant::now()));
            if self.gate.entered(slice).await.is_ok() {
                self.gate_entered = true;
                emit("gate_entered", json!({"phase": RECOVERY_GATE_PHASE}));
                return Ok(());
            }
            if tokio::time::Instant::now() >= until {
                return Err("the prepared recovery write was not reached".into());
            }
        }
    }

    async fn await_terminal(&mut self, scenario: &Scenario) -> Result<(), String> {
        let within = match scenario.end_mode {
            EndMode::Manual => Duration::from_secs(ENDING_S),
            EndMode::ServerDeadline => {
                let hard = super::super::super::interview_hard_deadline(SCENARIO_DURATION_MIN);
                let spent = self.ready_at.map_or(Duration::ZERO, |at| at.elapsed());
                hard.saturating_sub(spent) + Duration::from_secs(ENDING_S)
            }
            EndMode::Absence => {
                super::super::super::CANDIDATE_ABSENCE_LIMIT + Duration::from_secs(60)
            }
        };
        let terminal = self
            .wait_for(within, "the expected terminal", |this| {
                this.agent_finished.get().is_some()
            })
            .await;
        if let Some(handle) = self.agent.take() {
            if terminal.is_ok() {
                self.agent_end = Some(handle.await.unwrap_or(AgentEnd::Aborted));
            } else {
                self.agent = Some(handle);
            }
        }
        emit(
            "terminal",
            json!({"mode": scenario.end_mode.manifest_name(), "observed": terminal.is_ok()}),
        );
        terminal
    }

    async fn release_gate(&mut self) {
        self.gate.release();
    }
}

/// Wall time the scripted part of a run may take: the cap less the final
/// cleanup minute. The ending window is inside it, because the terminal wait
/// and the report belong to the script, not to the cleanup.
fn script_budget(scenario: &Scenario) -> Duration {
    Duration::from_secs(scenario.outer_cap_s - CLEANUP_S)
}

/// The scenario the controller selected for this invocation, by its letter.
fn scenario_for(letter: &str) -> Option<&'static Scenario> {
    SCENARIOS
        .iter()
        .find(|scenario| letter.len() == 1 && scenario.name.starts_with(letter))
}

/// Writes the manifest of one scenario for a pinned binary. It does nothing
/// unless the launcher wrapper (or the test below) asks for it with
/// `CODETRIAL_LIVE_MANIFEST_OUT`, so the ordinary lane passes it vacuously; it
/// takes no permit and touches no provider.
#[test]
fn live_write_manifest() {
    let Ok(out) = std::env::var("CODETRIAL_LIVE_MANIFEST_OUT") else {
        return;
    };
    let letter = std::env::var("CODETRIAL_LIVE_SCENARIO").expect("set CODETRIAL_LIVE_SCENARIO");
    let candidate = PathBuf::from(
        std::env::var("CODETRIAL_LIVE_CANDIDATE").expect("set CODETRIAL_LIVE_CANDIDATE"),
    );
    let scenario = scenario_for(&letter).expect("the scenario is one of a, b, c, d");
    let digest = crate::sha256_hex(&[&std::fs::read(&candidate).expect("read the pinned binary")]);
    let manifest = scenario.manifest(&candidate, &digest);
    std::fs::write(out, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
}

/// NOT RUN against LiveKit or Gemini. Hosts the real room loop in this
/// process, with a real SDK candidate in a real room and the configured
/// Gemini, under the B0.1 launcher that sets the permit variables. It spends
/// LiveKit and Gemini quota.
///
/// One scenario per invocation, chosen by `CODETRIAL_LIVE_SCENARIO` (a, b, c
/// or d) and checked against the manifest the launcher holds. Before any
/// permit it checks the variables and manifest, that the manifest names the
/// selected scenario with exactly its declared actions, ceiling, cap and end
/// mode, that the bootstrap built from the manifest has the hard deadline the
/// scenario observes, and that the candidate token is signed under the token
/// endpoint's own duration rules. Every side effect after that, including
/// starting the room loop, is behind a permit.
///
/// Real here: the LiveKit room and SDK transport on both ends, the candidate's
/// data packets, the interviewer's room loop and recovery driver, Gemini Live
/// and the report call. Not real: the speech (nothing is spoken; the editor
/// state is authored), the browser page (the receipt is the same digest the
/// page sends, built by `candidate_ack`), and the recovery trigger, which ends
/// the first Live reader so the loop takes its own Close path.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "NOT RUN: spends LiveKit and Gemini quota; needs the B0.1 launcher variables, CODETRIAL_LIVE_CONFIG, CODETRIAL_LIVE_SCENARIO and budget approval"]
async fn live_sdk_control_presence_and_report_delivery() {
    let mut permits = PermitClient::from_env()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let letter = std::env::var("CODETRIAL_LIVE_SCENARIO")
        .expect("set CODETRIAL_LIVE_SCENARIO to a, b, c or d");
    let scenario = scenario_for(&letter).expect("CODETRIAL_LIVE_SCENARIO is one of a, b, c, d");
    assert_eq!(
        permits.manifest["run_id"],
        json!(scenario.name.replace('_', "-")),
        "the manifest is for the selected scenario"
    );
    assert_eq!(
        permits.manifest["actions"],
        json!(scenario.actions),
        "the manifest lists the declared actions"
    );
    assert_eq!(
        permits.manifest["action_ceiling"],
        json!(scenario.ceiling),
        "the hard ceiling includes the reserved End"
    );
    assert_eq!(
        permits.manifest["outer_wall_cap_s"],
        json!(scenario.outer_cap_s)
    );
    assert_eq!(
        permits.manifest["end_mode"],
        json!(scenario.end_mode.manifest_name())
    );

    // The cap the server would apply to the token, from the same variable the
    // browser plan reads; unset means the product ceiling.
    let recording_cap = std::env::var("CODETRIAL_RECORDING_MAX_MINUTES")
        .ok()
        .map(|raw| raw.parse::<u32>().expect("a whole number of minutes"));
    let duration = duration_min_from_manifest(&permits.manifest, recording_cap)
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
    let boot = crate::runtime::bootstrap(&config, "live-recovery", Some("two-sum"), duration);
    assert_eq!(
        super::super::super::interview_hard_deadline(boot.duration_min).as_secs(),
        u64::from(duration) * 60 + 120
    );

    // Armed before the loop exists, so the loop that starts takes it.
    let gate = RecoveryGate::arm();
    let mut effects = LiveEffects::new(
        scenario,
        config.clone(),
        recording_cap,
        duration,
        &gate,
        permits.stop_file.clone(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(effects.wired());
    emit(
        "start",
        json!({"scenario": scenario.name, "room": effects.room_name, "duration_min": duration}),
    );

    // Not wrapped in a timeout: dropping the script between a grant and its
    // record would lose a spent permit. Every wait in it ends at `script_stop`.
    let result = run_script(&mut permits, scenario, &mut effects).await;
    gate.release();
    let cleanup = cleanup_end_if_needed(&mut permits, &mut effects, &result).await;
    let teardown = effects.teardown().await;
    let facts = effects.facts(&teardown);
    let problems = judge(scenario, &facts);
    let (ended_at, transient_leaves, receipts_exactly) = {
        let observed = effects.shared.observed.lock().unwrap();
        (
            observed.presence.ended_at().map(|at| at.as_secs_f64()),
            observed.presence.transient_leaves,
            observed.tally.exactly(scenario),
        )
    };
    emit(
        "result",
        json!({
            "scenario": scenario.name,
            "script": format!("{result:?}"),
            "cleanup_end": format!("{cleanup:?}"),
            "permits": permits.granted,
            "agent": format!("{:?}", facts.agent),
            "final_receipts": facts.final_copies,
            "receipts_exactly_as_declared": receipts_exactly,
            "distinct_final_reports": facts.final_distinct,
            "provisional_reports": facts.provisional,
            "ack_failures": facts.ack_failures,
            "gate_entered": facts.gate_entered,
            "end_while_pending": facts.end_while_pending,
            "agent_after_return": facts.agent_after_return,
            "transient_leaves": transient_leaves,
            "ended_at_s": ended_at,
            "deadline_elapsed_s": facts.deadline_elapsed_s,
            "absence_elapsed_s": facts.absence_elapsed_s,
            "participants_left": facts.participants_left,
            "room_gone_after_delete": facts.room_gone_after_delete,
            "readers_unfinished": facts.readers_unfinished,
            "teardown_notes": teardown.notes,
            "problems": problems,
        }),
    );
    if let Err(error) = cleanup {
        eprintln!("cleanup End failed: {error:?}");
    }
    result.expect("the scripted live run completed");
    assert!(problems.is_empty(), "{problems:?}");
}

// Local tests. Everything below runs in the ordinary lane.

/// Where the fixture's scratch files go, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ct-live-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// How the in-test service answers `permit`. `status` is always answered
/// faithfully so a client can join and the fault is the permit's.
#[derive(Clone, Copy, Debug)]
enum Reply {
    Faithful,
    NotJson,
    WrongVersion,
    Oversized,
    HangsUp,
    Silent,
    GrantsAnotherAction,
    GrantsAnotherSequence,
    Denies,
    NoVerdict,
}

/// A faithful in-test copy of the launcher's decision and framing, over a Unix
/// socket. It is not the launcher: `launcher_*` below drives the real one.
struct Identity {
    run_id: String,
    digest: String,
}

struct Service {
    socket: PathBuf,
    manifest: PathBuf,
    identity: Identity,
    permits: Arc<AtomicUsize>,
    connections: Arc<AtomicUsize>,
    events: Arc<std::sync::Mutex<Vec<String>>>,
    _task: tokio::task::JoinHandle<()>,
}

/// Starts the service for `manifest`, written where the worker reads it, with
/// the digest the launcher would have computed over those bytes.
fn start_service(
    scratch: &Scratch,
    reply: Reply,
    allowed: &[&str],
    ceiling: usize,
    manifest: &Value,
) -> Service {
    let socket = scratch.path("permit.sock");
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let raw = serde_json::to_vec(manifest).unwrap();
    let manifest_path = scratch.path("run-manifest.json");
    std::fs::write(&manifest_path, &raw).unwrap();
    let identity = Identity {
        run_id: manifest["run_id"].as_str().unwrap().to_owned(),
        digest: crate::sha256_hex(&[&raw]),
    };
    let permits = Arc::new(AtomicUsize::new(0));
    let connections = Arc::new(AtomicUsize::new(0));
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let granted = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let allowed: Vec<String> = allowed.iter().map(|id| (*id).to_owned()).collect();
    let (run_id, digest) = (identity.run_id.clone(), identity.digest.clone());
    let task = tokio::spawn({
        let (permits, connections, events) = (permits.clone(), connections.clone(), events.clone());
        async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                connections.fetch_add(1, Ordering::SeqCst);
                let (permits, events, granted) = (permits.clone(), events.clone(), granted.clone());
                let (allowed, run_id, digest) = (allowed.clone(), run_id.clone(), digest.clone());
                tokio::spawn(async move {
                    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
                    let mut line = String::new();
                    let mut reader = tokio::io::BufReader::new(&mut stream);
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                    let faithful = |request: &Value| {
                        let mut granted = granted.lock().unwrap();
                        if request["v"] != 1
                            || request["run_id"] != run_id
                            || request["manifest_sha256"] != digest
                        {
                            return json!({"v": 1, "granted": false, "reason": "run_identity_mismatch"});
                        }
                        match request["op"].as_str() {
                            Some("status") => {
                                json!({"v": 1, "ok": true, "phase": "open", "count": granted.len()})
                            }
                            Some("permit") => {
                                let action = request["action_id"].as_str().unwrap_or_default();
                                let seq = request["seq"].as_u64();
                                if !allowed.iter().any(|id| id == action) {
                                    json!({"v": 1, "granted": false, "reason": "unknown_action"})
                                } else if granted.iter().any(|id| id == action) {
                                    json!({"v": 1, "granted": false, "reason": "duplicate_action"})
                                } else if seq != Some(granted.len() as u64 + 1)
                                    || granted.len() + 1 > ceiling
                                {
                                    json!({"v": 1, "granted": false, "reason": "sequence_mismatch"})
                                } else {
                                    granted.push(action.to_owned());
                                    json!({"v": 1, "granted": true, "action_id": action, "seq": seq})
                                }
                            }
                            _ => json!({"v": 1, "granted": false, "reason": "malformed_request"}),
                        }
                    };
                    let op = request["op"].as_str().unwrap_or_default().to_owned();
                    if op == "permit" {
                        permits.fetch_add(1, Ordering::SeqCst);
                        events.lock().unwrap().push(format!(
                            "permit:{}",
                            request["action_id"].as_str().unwrap_or_default()
                        ));
                    }
                    let verdict = faithful(&request);
                    let body: Vec<u8> = if op != "permit" {
                        format!("{verdict}\n").into_bytes()
                    } else {
                        match reply {
                            Reply::Faithful => format!("{verdict}\n").into_bytes(),
                            Reply::NotJson => b"not json\n".to_vec(),
                            Reply::WrongVersion => b"{\"v\":2,\"granted\":true}\n".to_vec(),
                            Reply::Oversized => {
                                // A grant that would be exact if read whole.
                                let mut padded = verdict.clone();
                                padded["pad"] = json!("x".repeat(MAX_REPLY_BYTES + 1));
                                format!("{padded}\n").into_bytes()
                            }
                            Reply::HangsUp => return,
                            Reply::Silent => {
                                tokio::time::sleep(Duration::from_secs(5)).await;
                                return;
                            }
                            Reply::GrantsAnotherAction => {
                                let mut other = verdict.clone();
                                other["action_id"] = json!("another_action");
                                format!("{other}\n").into_bytes()
                            }
                            Reply::GrantsAnotherSequence => {
                                let mut other = verdict.clone();
                                other["seq"] = json!(99);
                                format!("{other}\n").into_bytes()
                            }
                            Reply::Denies => {
                                b"{\"v\":1,\"granted\":false,\"reason\":\"draining\"}\n".to_vec()
                            }
                            Reply::NoVerdict => b"{\"v\":1}\n".to_vec(),
                        }
                    };
                    let _ = stream.write_all(&body).await;
                });
            }
        }
    });
    Service {
        socket,
        manifest: manifest_path,
        identity,
        permits,
        connections,
        events,
        _task: task,
    }
}

/// The variables the launcher would give a worker for this service.
fn variables(service: &Service) -> impl Fn(&str) -> Option<String> + use<> {
    let values = std::collections::HashMap::from([
        (
            "BROWSER_CHECK_RUN_MANIFEST",
            service.manifest.to_str().unwrap().to_owned(),
        ),
        ("BROWSER_CHECK_RUN_ID", service.identity.run_id.clone()),
        (
            "BROWSER_CHECK_RUN_MANIFEST_SHA256",
            service.identity.digest.clone(),
        ),
        (
            "BROWSER_CHECK_BUDGET_SOCKET",
            service.socket.to_str().unwrap().to_owned(),
        ),
        (
            "BROWSER_CHECK_BUDGET_STOP",
            stop_file(service).to_str().unwrap().to_owned(),
        ),
    ]);
    move |name| values.get(name).cloned()
}

fn stop_file(service: &Service) -> PathBuf {
    service.manifest.with_file_name("stop")
}

/// Declared before running: the launcher's stop request is the stop file
/// appearing, and it is read, not assumed.
#[tokio::test]
async fn the_stop_request_is_the_launchers_stop_file() {
    let (_scratch, service, client) = joined(Reply::Faithful, &["candidate_join"]).await;
    assert!(!client.stop_requested());
    std::fs::write(stop_file(&service), "").unwrap();
    assert!(client.stop_requested());
}

fn simple_manifest(actions: &[&str]) -> Value {
    json!({"run_id": "run-test", "actions": actions})
}

async fn joined(reply: Reply, actions: &[&str]) -> (Scratch, Service, PermitClient) {
    let scratch = Scratch::new("permit");
    let mut allowed = actions.to_vec();
    allowed.push(FINAL_ACTION_ID);
    let manifest = simple_manifest(actions);
    let service = start_service(&scratch, reply, &allowed, actions.len() + 1, &manifest);
    let client = PermitClient::from_variables(variables(&service))
        .await
        .unwrap();
    (scratch, service, client)
}

/// Declared before running: every scripted action is granted once, in order,
/// at the next sequence number, and the reserved End is granted last.
#[tokio::test]
async fn the_permit_client_is_granted_each_scripted_action_once_in_order() {
    let actions = ["candidate_join", "mic_1", "edit_1"];
    let (_scratch, service, mut client) = joined(Reply::Faithful, &actions).await;
    for action in actions.iter().copied().chain([FINAL_ACTION_ID]) {
        client.permit(action).await.unwrap();
    }
    assert_eq!(client.granted, ["candidate_join", "mic_1", "edit_1", "end"]);
    assert_eq!(service.permits.load(Ordering::SeqCst), 4);

    // A granted action asked for again is refused without asking, and that
    // refusal kills the client.
    let before = service.connections.load(Ordering::SeqCst);
    assert!(matches!(
        client.permit("mic_1").await,
        Err(PermitError::Denied(_))
    ));
    assert!(matches!(
        client.permit("edit_1").await,
        Err(PermitError::Dead(_))
    ));
    assert_eq!(service.connections.load(Ordering::SeqCst), before);
}

/// Declared before running: a second client joining mid-run starts at the
/// launcher's count, so a restarted worker cannot spend an action again.
#[tokio::test]
async fn a_client_joining_mid_run_continues_from_the_launchers_count() {
    let actions = ["candidate_join", "mic_1", "edit_1"];
    let (_scratch, service, mut first) = joined(Reply::Faithful, &actions).await;
    first.permit("candidate_join").await.unwrap();
    first.permit("mic_1").await.unwrap();
    let mut second = PermitClient::from_variables(variables(&service))
        .await
        .unwrap();
    assert_eq!(second.next_seq, 3);
    // The action the first client already spent is refused by the service.
    assert!(second.permit("mic_1").await.is_err());
    let mut third = PermitClient::from_variables(variables(&service))
        .await
        .unwrap();
    third.permit("edit_1").await.unwrap();
}

/// Declared before running: each reply that is not an exact grant for this
/// action and sequence fails closed with no effect, the failure is not retried
/// (one permit connection), and the dead client then refuses without asking.
#[tokio::test]
async fn the_permit_client_fails_closed_on_every_reply_that_is_not_an_exact_grant() {
    for (reply, takes_at_least) in [
        (Reply::NotJson, Duration::ZERO),
        (Reply::WrongVersion, Duration::ZERO),
        (Reply::Oversized, Duration::ZERO),
        (Reply::HangsUp, Duration::ZERO),
        (Reply::GrantsAnotherAction, Duration::ZERO),
        (Reply::GrantsAnotherSequence, Duration::ZERO),
        (Reply::Denies, Duration::ZERO),
        (Reply::NoVerdict, Duration::ZERO),
        (Reply::Silent, PERMIT_TIMEOUT),
    ] {
        let actions = ["candidate_join", "mic_1"];
        let (_scratch, service, mut client) = joined(reply, &actions).await;
        let dispatched = AtomicUsize::new(0);
        let started = std::time::Instant::now();
        let outcome = client
            .guarded("candidate_join", async {
                dispatched.fetch_add(1, Ordering::SeqCst);
            })
            .await;
        assert!(outcome.is_err(), "{reply:?} was treated as granted");
        assert_eq!(dispatched.load(Ordering::SeqCst), 0, "{reply:?} dispatched");
        assert!(started.elapsed() >= takes_at_least, "{reply:?}");
        assert!(
            started.elapsed() < PERMIT_TIMEOUT + Duration::from_millis(900),
            "{reply:?} outlived the permit timeout: {:?}",
            started.elapsed()
        );
        assert_eq!(
            service.permits.load(Ordering::SeqCst),
            1,
            "{reply:?} retried"
        );
        let connections = service.connections.load(Ordering::SeqCst);
        assert!(client.permit("mic_1").await.is_err(), "{reply:?}");
        assert!(client.guarded("mic_1", async {}).await.is_err());
        assert_eq!(
            service.connections.load(Ordering::SeqCst),
            connections,
            "{reply:?}: a dead client contacted the launcher"
        );
        assert!(client.granted.is_empty());
    }
}

/// Declared before running: a launcher that is gone (no socket) fails closed
/// as a lost launcher, not a denial, and a duplicate action is refused by the
/// client before it asks.
#[tokio::test]
async fn an_unreachable_launcher_and_a_duplicate_action_are_refused() {
    let actions = ["candidate_join", "mic_1"];
    let (scratch, service, mut client) = joined(Reply::Faithful, &actions).await;
    client.permit("candidate_join").await.unwrap();
    let permits = service.permits.load(Ordering::SeqCst);
    assert!(matches!(
        client.permit("candidate_join").await,
        Err(PermitError::Denied(_))
    ));
    assert_eq!(service.permits.load(Ordering::SeqCst), permits);

    let gone = Scratch::new("gone");
    let mut stranded = client;
    stranded.dead = None;
    stranded.socket = gone.path("missing.sock");
    assert!(matches!(
        stranded.permit("mic_1").await,
        Err(PermitError::Lost(_))
    ));
    drop(scratch);
}

/// Declared before running: the variables and the manifest are refused unless
/// all four are present, the digest is the manifest's, and the run ID matches.
#[tokio::test]
async fn the_permit_client_refuses_an_incomplete_or_mismatched_environment() {
    let scratch = Scratch::new("env");
    let manifest = simple_manifest(&["candidate_join"]);
    let service = start_service(
        &scratch,
        Reply::Faithful,
        &["candidate_join", "end"],
        2,
        &manifest,
    );
    let complete = variables(&service);
    PermitClient::from_variables(&complete).await.unwrap();

    for name in BUDGET_VARIABLES {
        let result = PermitClient::from_variables(|asked| {
            (asked != name).then(|| complete(asked)).flatten()
        })
        .await;
        assert!(
            matches!(result, Err(PermitError::Setup(ref why)) if why.contains("incomplete")),
            "missing {name}"
        );
    }
    let digest = PermitClient::from_variables(|name| {
        if name == "BROWSER_CHECK_RUN_MANIFEST_SHA256" {
            Some("0".repeat(64))
        } else {
            complete(name)
        }
    })
    .await;
    assert!(matches!(digest, Err(PermitError::Setup(ref why)) if why.contains("hash")));
    let other_run = PermitClient::from_variables(|name| {
        if name == "BROWSER_CHECK_RUN_ID" {
            Some("run-other".into())
        } else {
            complete(name)
        }
    })
    .await;
    assert!(matches!(other_run, Err(PermitError::Setup(ref why)) if why.contains("run ID")));
}

/// Declared before running: the dispatch of a guarded action does not start
/// before its grant is received, and a refusal leaves it unstarted.
#[tokio::test]
async fn the_permit_is_taken_before_the_side_effect() {
    let (_scratch, service, mut client) = joined(Reply::Faithful, &["candidate_join"]).await;
    let events = service.events.clone();
    client
        .guarded("candidate_join", async {
            events.lock().unwrap().push("effect:candidate_join".into());
        })
        .await
        .unwrap();
    assert_eq!(
        *service.events.lock().unwrap(),
        ["permit:candidate_join", "effect:candidate_join"]
    );

    let (_scratch, service, mut denied) = joined(Reply::Denies, &["candidate_join"]).await;
    let events = service.events.clone();
    let outcome = denied
        .guarded("candidate_join", async {
            events.lock().unwrap().push("effect:candidate_join".into());
        })
        .await;
    assert!(outcome.is_err());
    assert_eq!(*service.events.lock().unwrap(), ["permit:candidate_join"]);
}

/// The launcher's own service, over a fake worker that is this test binary.
///
/// Declared before running: the real `scripts/interview-run-budget.py run`
/// starts this binary on the one test below that is the worker; the worker
/// reads the launcher's variables, takes each manifest action and then End
/// through `PermitClient`, and dispatches by appending to a log. The ledger the
/// launcher wrote and the dispatch log agree, in order, and the launcher
/// reports a passing budget.
#[test]
fn launcher_grants_exactly_what_the_native_worker_dispatches() {
    let scratch = Scratch::new("launcher");
    let outcome = run_launcher(&scratch, "normal", &["candidate_join", "mic_1", "edit_1"]);
    assert!(outcome.status.success(), "{}", outcome.stderr);
    let dispatched = std::fs::read_to_string(scratch.path("dispatch.log")).unwrap();
    assert_eq!(
        dispatched.split_whitespace().collect::<Vec<_>>(),
        ["candidate_join", "mic_1", "edit_1", "end"]
    );
    let ledger = std::fs::read_to_string(scratch.path("out/budget-ledger.jsonl")).unwrap();
    let granted: Vec<String> = ledger
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["event"] == "grant")
        .map(|event| event["action_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(granted, ["candidate_join", "mic_1", "edit_1", "end"]);
}

/// Declared before running: a worker that asks the launcher for an action it
/// already spent is refused as a duplicate, the launcher records the fault and
/// fails the run, and each action was dispatched once.
#[test]
fn launcher_refuses_and_records_a_repeated_action() {
    let scratch = Scratch::new("launcher-dup");
    let outcome = run_launcher(&scratch, "duplicate", &["candidate_join", "mic_1"]);
    assert!(!outcome.status.success(), "{}", outcome.stderr);
    let dispatched = std::fs::read_to_string(scratch.path("dispatch.log")).unwrap();
    assert_eq!(
        dispatched.split_whitespace().collect::<Vec<_>>(),
        ["candidate_join", "mic_1"]
    );
    let denied = std::fs::read_to_string(scratch.path("denied.log")).unwrap();
    assert!(
        denied.contains("\"granted\":false") && denied.contains("duplicate_action"),
        "{denied}"
    );
    let ledger = std::fs::read_to_string(scratch.path("out/budget-ledger.jsonl")).unwrap();
    assert!(
        ledger
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .any(|event| event["event"] == "fault" && event["reason"] == "duplicate_action"),
        "{ledger}"
    );
}

struct LauncherRun {
    status: std::process::ExitStatus,
    stderr: String,
}

fn child_test_name() -> String {
    // libtest names a test without its crate.
    let module = module_path!().split_once("::").unwrap().1;
    format!("{module}::permit_worker_child")
}

fn run_launcher(scratch: &Scratch, mode: &str, actions: &[&str]) -> LauncherRun {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let candidate = scratch.path("candidate");
    std::fs::write(&candidate, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = std::fs::metadata(&candidate).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&candidate, permissions).unwrap();
    let manifest = json!({
        "schema_version": 1,
        "run_id": "run-native-1",
        "binaries": {"candidate": {
            "path": candidate.to_str().unwrap(),
            "sha256": crate::sha256_hex(&[&std::fs::read(&candidate).unwrap()]),
        }},
        "settings_sha256": "a".repeat(64),
        "fixture_sha256": "a".repeat(64),
        "selected_duration_s": 10,
        "outer_wall_cap_s": 30,
        "cleanup_reserve_s": 4,
        "end_mode": "explicit_end",
        "actions": actions,
        "action_ceiling": actions.len() + 1,
        "limits": {},
        "scopes": {"scripted_actions": "hard", "wall_time": "hard"},
    });
    let manifest_path = scratch.path("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let output = std::process::Command::new("python3")
        .arg(root.join("scripts/interview-run-budget.py"))
        .args(["run", "--manifest"])
        .arg(&manifest_path)
        .arg("--artifact-dir")
        .arg(scratch.path("out"))
        .args([
            "--allow-short-reserve",
            "1",
            "--sample-interval",
            "0.05",
            "--reap-budget",
            "1.5",
            "--",
        ])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", &child_test_name(), "--nocapture"])
        .env("CODETRIAL_PERMIT_WORKER_DIR", &scratch.0)
        .env("CODETRIAL_PERMIT_WORKER_MODE", mode)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("python3 runs the launcher");
    LauncherRun {
        status: output.status,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// The worker half of the two launcher tests. It does nothing unless the
/// launcher test started it with a scratch directory, so the ordinary lane
/// passes it vacuously and it is not a test of anything on its own.
#[tokio::test]
async fn permit_worker_child() {
    let Ok(dir) = std::env::var("CODETRIAL_PERMIT_WORKER_DIR") else {
        return;
    };
    let mode = std::env::var("CODETRIAL_PERMIT_WORKER_MODE").unwrap();
    let dir = PathBuf::from(dir);
    let note = |name: &str, text: &str| {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(name))
            .unwrap();
        writeln!(file, "{text}").unwrap();
    };
    let mut client = PermitClient::from_env().await.unwrap();
    let actions: Vec<String> = client.manifest["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|action| action.as_str().unwrap().to_owned())
        .collect();
    let mut script: Vec<String> = actions.clone();
    if mode != "duplicate" {
        script.push(FINAL_ACTION_ID.into());
    }
    for action in script {
        match client
            .guarded(&action, async { note("dispatch.log", &action) })
            .await
        {
            Ok(()) => {}
            Err(error) => {
                note("denied.log", &error.to_string());
                std::process::exit(3);
            }
        }
    }
    if mode == "duplicate" {
        // The client refuses a repeat itself, so the launcher is asked
        // directly: its own refusal and fault record are what is under test.
        let seq = client.next_seq;
        let reply = client
            .call(json!({"op": "permit", "action_id": actions[0], "seq": seq}))
            .await
            .expect("the launcher answers");
        note("denied.log", &reply.to_string());
        std::process::exit(3);
    }
}

/// Declared before running: the four action lists are the ones the task
/// declares, with the ceilings 6/4/5/6 (the reserved End included), End
/// scripted in (a) and (d) only.
#[test]
fn the_four_scenarios_declare_the_explicit_action_lists_and_ceilings() {
    let scripts: Vec<Vec<&str>> = SCENARIOS.iter().map(Scenario::script).collect();
    assert_eq!(
        scripts,
        [
            vec![
                "candidate_join",
                "seed_fixture",
                "controlled_recovery_trigger",
                "candidate_leave",
                "candidate_rejoin",
                "end"
            ],
            vec![
                "candidate_join",
                "seed_fixture",
                "controlled_recovery_trigger"
            ],
            vec![
                "candidate_join",
                "seed_fixture",
                "controlled_recovery_trigger",
                "candidate_leave"
            ],
            vec![
                "candidate_join",
                "seed_fixture",
                "thinking_hold",
                "controlled_recovery_trigger",
                "thinking_release",
                "end"
            ],
        ]
    );
    assert_eq!(
        SCENARIOS.iter().map(|s| s.ceiling).collect::<Vec<_>>(),
        [6, 4, 5, 6]
    );
    for scenario in &SCENARIOS {
        assert_eq!(
            scenario.ceiling,
            scenario.actions.len() + 1,
            "{}",
            scenario.name
        );
        assert!(!scenario.actions.contains(&FINAL_ACTION_ID));
    }
    assert_eq!(
        SCENARIOS.iter().map(|s| s.report).collect::<Vec<_>>(),
        [true, true, false, true]
    );
}

/// Declared before running: the wall budgets are the declared ones. The three
/// non-deadline cases take setup + observation + ending + cleanup = 660s under
/// the 15 minute cap; the deadline case is 2 + 12 + 5 + 1 = 20 minutes under a
/// 20 minute cap. The absence grace and the loop's tick fit the 180s
/// observation, and the transient return is well before the grace.
#[test]
fn the_scenario_budgets_fit_their_caps() {
    for scenario in &SCENARIOS {
        let total = SETUP_S + scenario.observation_s() + ENDING_S + CLEANUP_S;
        match scenario.end_mode {
            EndMode::ServerDeadline => {
                assert_eq!(scenario.observation_s(), 12 * 60);
                assert_eq!(total, 20 * 60);
                assert_eq!(scenario.outer_cap_s, 20 * 60);
            }
            EndMode::Manual | EndMode::Absence => {
                assert_eq!(total, 660);
                assert!(total < 15 * 60);
                assert_eq!(scenario.outer_cap_s, 15 * 60);
            }
        }
        assert!(
            scenario.selected_duration_s() + scenario.cleanup_reserve_s() <= scenario.outer_cap_s,
            "{}",
            scenario.name
        );
    }
    let absence =
        super::super::super::CANDIDATE_ABSENCE_LIMIT.as_secs_f64() + crate::agent::WATCH_TICK_S;
    assert!(absence < OBSERVATION_S as f64, "{absence}");
    assert!(RETURN_WITHIN_S < super::super::super::CANDIDATE_ABSENCE_LIMIT.as_secs());
}

/// Declared before running: the controller accepts the manifest each scenario
/// builds. The controller's own `validate` is the judge, not a copy of its
/// rules.
#[test]
fn the_launcher_validates_each_scenarios_manifest() {
    let scratch = Scratch::new("manifests");
    let candidate = scratch.path("candidate");
    std::fs::write(&candidate, "x").unwrap();
    for scenario in &SCENARIOS {
        let manifest = scenario.manifest(&candidate, &"c".repeat(64));
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
            scenario.name,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            duration_min_from_manifest(&manifest, None),
            Ok(SCENARIO_DURATION_MIN),
            "{}",
            scenario.name
        );
    }
}

/// Declared before running: the bootstrap duration comes from the validated
/// manifest by the T8a.1 rules, and the real hard-deadline helper turns the
/// server-deadline case's 10 minutes into 12 minutes.
#[test]
fn the_deadline_scenario_bootstrap_has_a_twelve_minute_hard_deadline() {
    let scratch = Scratch::new("deadline");
    let candidate = scratch.path("candidate");
    std::fs::write(&candidate, "x").unwrap();
    let manifest = SCENARIOS[1].manifest(&candidate, &"c".repeat(64));
    let duration = duration_min_from_manifest(&manifest, None).unwrap();
    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let boot = crate::runtime::bootstrap(&config, "live-recovery", Some("two-sum"), duration);
    assert_eq!(boot.duration_min, 10);
    assert_eq!(
        super::super::super::interview_hard_deadline(boot.duration_min),
        Duration::from_secs(12 * 60)
    );
    assert_eq!(manifest["observation_target_s"], 12 * 60);
}

/// Declared before running: the duration rules are T8a.1's. Outside 10..=90,
/// a fraction, a string, a missing field, a recording cap below the request, a
/// deadline run whose observation is not the server's deadline and a manual run
/// observing longer than its interview are all refused; a cap below the floor
/// clamps to the floor rather than panicking, as the token endpoint does.
#[test]
fn the_manifest_duration_follows_the_product_range_and_the_recording_cap() {
    let base = |end_mode: &str, minutes: Value, target: u64| json!({"end_mode": end_mode, "duration_min": minutes, "observation_target_s": target});
    for (manifest, cap, expected) in [
        (base("server_deadline", json!(10), 720), None, Ok(10)),
        (
            base("server_deadline", json!(90), 90 * 60 + 120),
            None,
            Ok(90),
        ),
        (base("manual", json!(10), 180), None, Ok(10)),
        (base("absence", json!(10), 600), None, Ok(10)),
        (base("manual", json!(10), 180), Some(5), Ok(10)),
        (base("manual", json!(20), 180), Some(15), Err(())),
        (base("manual", json!(9), 180), None, Err(())),
        (base("manual", json!(91), 180), None, Err(())),
        (base("manual", json!(10.5), 180), None, Err(())),
        (base("manual", json!("10"), 180), None, Err(())),
        (base("manual", json!(0), 180), None, Err(())),
        (base("manual", json!(10), 601), None, Err(())),
        (base("server_deadline", json!(10), 600), None, Err(())),
        (base("server_deadline", json!(10), 721), None, Err(())),
        (base("explicit_end", json!(10), 180), None, Err(())),
        (
            json!({"end_mode": "manual", "observation_target_s": 180}),
            None,
            Err(()),
        ),
    ] {
        assert_eq!(
            duration_min_from_manifest(&manifest, cap).map_err(|_| ()),
            expected,
            "{manifest} cap {cap:?}"
        );
    }
}

/// Records what a script did, and answers the barriers.
#[derive(Default)]
struct ScriptedEffects {
    log: Vec<String>,
    gate_entered_after_trigger: bool,
}

impl Effects for ScriptedEffects {
    async fn perform(&mut self, action: &str) -> Result<(), String> {
        self.log.push(format!("effect:{action}"));
        Ok(())
    }
    async fn await_gate_entered(&mut self) -> Result<(), String> {
        assert_eq!(
            self.log.last().map(String::as_str),
            Some("effect:controlled_recovery_trigger"),
            "the gate is awaited right after the trigger"
        );
        self.gate_entered_after_trigger = true;
        self.log.push("barrier:gate_entered".into());
        Ok(())
    }
    async fn await_terminal(&mut self, scenario: &Scenario) -> Result<(), String> {
        assert!(self.gate_entered_after_trigger);
        self.log
            .push(format!("barrier:terminal:{:?}", scenario.end_mode));
        Ok(())
    }
    async fn release_gate(&mut self) {
        assert!(
            self.log
                .iter()
                .any(|entry| entry.starts_with("barrier:terminal")),
            "the gate is retired only after the expected terminal"
        );
        self.log.push("release_gate".into());
    }
}

/// Declared before running: each scenario's script takes one permit before each
/// of its effects, in order, within its ceiling; the gate is awaited after the
/// trigger and released only after the terminal; barriers take no permit.
#[tokio::test]
async fn each_scenario_script_takes_a_permit_before_each_effect_within_its_ceiling() {
    for scenario in &SCENARIOS {
        let scratch = Scratch::new("script");
        let mut allowed = scenario.actions.to_vec();
        allowed.push(FINAL_ACTION_ID);
        let manifest = simple_manifest(scenario.actions);
        let service = start_service(
            &scratch,
            Reply::Faithful,
            &allowed,
            scenario.ceiling,
            &manifest,
        );
        let mut permits = PermitClient::from_variables(variables(&service))
            .await
            .unwrap();
        let mut effects = ScriptedEffects::default();
        run_script(&mut permits, scenario, &mut effects)
            .await
            .unwrap_or_else(|error| panic!("{}: {error:?}", scenario.name));

        let script = scenario.script();
        assert_eq!(permits.granted, script, "{}", scenario.name);
        assert!(permits.granted.len() <= scenario.ceiling);
        let effects_done: Vec<_> = effects
            .log
            .iter()
            .filter_map(|entry| entry.strip_prefix("effect:"))
            .collect();
        assert_eq!(effects_done, script, "{}", scenario.name);
        assert_eq!(service.permits.load(Ordering::SeqCst), script.len());
        assert_eq!(effects.log.last().map(String::as_str), Some("release_gate"));
        // Automatic terminals spend no End; cleanup may still ask for it once.
        if !scenario.scripted_end {
            cleanup_end(&mut permits, &mut effects).await.unwrap();
            assert_eq!(permits.granted.last().map(String::as_str), Some("end"));
            assert!(permits.granted.len() <= scenario.ceiling);
        } else {
            assert!(cleanup_end(&mut permits, &mut effects).await.is_err());
            assert_eq!(effects.log.iter().filter(|e| *e == "effect:end").count(), 1);
        }
    }
}

/// Declared before running: when the launcher refuses the second action, the
/// first effect happened, the second did not, and the script ends in a permit
/// error.
#[tokio::test]
async fn a_refused_permit_stops_the_script_before_its_effect() {
    let scenario = &SCENARIOS[1];
    let scratch = Scratch::new("refused");
    // Only the first action and End are allowed; seed_fixture is unknown.
    let manifest = simple_manifest(scenario.actions);
    let service = start_service(
        &scratch,
        Reply::Faithful,
        &["candidate_join", "end"],
        scenario.ceiling,
        &manifest,
    );
    let mut permits = PermitClient::from_variables(variables(&service))
        .await
        .unwrap();
    let mut effects = ScriptedEffects::default();
    let result = run_script(&mut permits, scenario, &mut effects).await;
    assert!(matches!(result, Err(ScriptError::Permit(_))), "{result:?}");
    assert_eq!(effects.log, ["effect:candidate_join"]);
}

/// Declared before running: the unwired live effects stop the very first
/// permitted step with an error, and nothing past it runs.
#[tokio::test]
async fn the_unwired_live_effects_stop_the_script_at_the_first_action() {
    let scenario = &SCENARIOS[0];
    let (_scratch, service, mut permits) = joined(Reply::Faithful, scenario.actions).await;
    let result = run_script(&mut permits, scenario, &mut UnwiredEffects).await;
    assert!(
        matches!(result, Err(ScriptError::Effect(ref why)) if why.starts_with("NOT RUN")),
        "{result:?}"
    );
    assert_eq!(service.permits.load(Ordering::SeqCst), 1);
}

/// Declared before running: once the launcher's stop file exists no further
/// action starts, whichever action is next, and cleanup asks for the reserved
/// End exactly once when the candidate had joined and End was not yet spent.
#[tokio::test]
async fn a_stop_request_starts_no_action_and_cleanup_spends_end_once() {
    let scenario = &SCENARIOS[0];
    let (_scratch, service, mut permits) = joined(Reply::Faithful, scenario.actions).await;
    let mut effects = ScriptedEffects::default();
    permits
        .guarded("candidate_join", effects.perform("candidate_join"))
        .await
        .unwrap()
        .unwrap();
    std::fs::write(stop_file(&service), "").unwrap();
    let result = run_script(&mut permits, scenario, &mut effects).await;
    assert_eq!(result, Err(ScriptError::Stopped));
    assert_eq!(effects.log, ["effect:candidate_join"]);
    assert_eq!(service.permits.load(Ordering::SeqCst), 1);

    cleanup_end_if_needed(&mut permits, &mut effects, &result)
        .await
        .unwrap();
    assert_eq!(effects.log, ["effect:candidate_join", "effect:end"]);
    cleanup_end_if_needed(&mut permits, &mut effects, &result)
        .await
        .unwrap();
    assert_eq!(effects.log.len(), 2, "End is spent once");
    assert_eq!(permits.granted, ["candidate_join", "end"]);

    // A run whose candidate never joined has no room to end.
    let (_scratch, service, mut unjoined) = joined(Reply::Faithful, scenario.actions).await;
    let mut effects = ScriptedEffects::default();
    cleanup_end_if_needed(&mut unjoined, &mut effects, &Err(ScriptError::Stopped))
        .await
        .unwrap();
    assert!(effects.log.is_empty());
    assert_eq!(service.permits.load(Ordering::SeqCst), 0);
}

/// Declared before running: the armed gate reaches the queue that
/// `run_room_with_slot` fills its `RoomLoop` from, with the recovery write's
/// phase, and the handles are the fixture's. This checks the hand-off only; it
/// does not run a room loop, so that the loop honours the gate is covered by
/// the recovery matrices on a hand-built loop, not here.
#[tokio::test]
async fn the_armed_recovery_gate_reaches_the_room_loops_barrier_queue() {
    let gate = RecoveryGate::arm();
    let queued = super::super::super::take_live_write_barriers();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].phase, RECOVERY_GATE_PHASE);
    assert!(
        super::super::super::take_live_write_barriers().is_empty(),
        "a loop takes the gates once"
    );
    assert!(gate.entered(Duration::from_millis(20)).await.is_err());
    queued[0].entered.notify_one();
    gate.entered(Duration::from_secs(1)).await.unwrap();
    let released = queued[0].release.clone();
    let waiter = tokio::spawn(async move { released.notified().await });
    tokio::task::yield_now().await;
    gate.release();
    tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("release wakes the holder")
        .unwrap();
}

/// Declared before running: the tally acknowledges a report once per received
/// copy, counts distinct final reports by their bytes, and does not count a
/// provisional report or a non-report. A report-producing case needs exactly
/// one distinct final report and the others none.
#[test]
fn the_report_tally_counts_distinct_final_reports() {
    let final_report = br#"{"codingScore":80}"#;
    let provisional = br#"{"reportRecovery":{"cause":"unavailable"}}"#;
    let mut tally = ReportTally::default();
    assert!(tally.observe(Some("control"), final_report).is_none());
    assert!(tally.observe(Some("report"), b"invalid").is_none());
    assert_eq!(tally.receipts, 0);
    assert!(!tally.matches(&SCENARIOS[0]) && tally.matches(&SCENARIOS[2]));

    assert!(tally.observe(Some("report"), provisional).is_some());
    assert_eq!((tally.provisional, tally.final_ids.len()), (1, 0));
    assert!(tally.observe(Some("report"), final_report).is_some());
    assert!(tally.observe(Some("report"), final_report).is_some());
    assert_eq!((tally.receipts, tally.final_ids.len()), (3, 1));
    assert!(tally.matches(&SCENARIOS[0]) && !tally.matches(&SCENARIOS[2]));
    assert!(
        tally
            .observe(Some("report"), br#"{"codingScore":81}"#)
            .is_some()
    );
    assert!(
        !tally.matches(&SCENARIOS[0]),
        "two distinct finals are not one"
    );
}

// Offline checks of the live fixture's own logic: the observer, the receipt
// count, the verdict, the redaction and the manifest writer.

fn at(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// Declared before running: a candidate leaving, closing its own connection and
/// returning is a sighting of the candidate and ends nothing. Only the
/// interviewer leaving, its task finishing or the server closing the room is an
/// ending, and a candidate who never returns still ends nothing by itself.
#[test]
fn a_transient_leave_is_not_an_ending() {
    let mut seen = PresenceObserver::default();
    seen.note(at(1), Sighting::Control("interviewer_state".into()));
    seen.note(at(2), Sighting::CandidateLeft);
    seen.note(at(2), Sighting::OwnClose);
    assert!(!seen.ended(), "a leave in progress is not an ending");
    seen.note(at(12), Sighting::CandidateReturned);
    assert!(!seen.ended(), "a return is not an ending");
    assert_eq!((seen.transient_leaves, seen.candidate_absent), (1, false));

    let mut gone = PresenceObserver::default();
    gone.note(at(2), Sighting::CandidateLeft);
    gone.note(at(2), Sighting::OwnClose);
    assert!(!gone.ended(), "absence alone is not an ending");
    assert_eq!(gone.transient_leaves, 0);
    gone.note(at(95), Sighting::AgentTaskFinished);
    assert!(gone.ended());
    assert_eq!(gone.ended_at(), Some(at(95)));

    for ending in [
        Sighting::AgentLeft,
        Sighting::AgentTaskFinished,
        Sighting::RoomClosed("RoomDeleted".into()),
    ] {
        let mut seen = PresenceObserver::default();
        seen.note(at(1), ending.clone());
        assert!(seen.ended(), "{ending:?}");
    }
    let mut first = PresenceObserver::default();
    first.note(at(30), Sighting::AgentTaskFinished);
    first.note(at(20), Sighting::AgentLeft);
    first.note(at(40), Sighting::AgentLeft);
    assert_eq!(first.ended_at(), Some(at(20)), "the earliest ending wins");
}

/// Declared before running: a second copy of the same final report is a second
/// receipt, so it is not the exactly-one a report case needs, while the
/// distinct-report count alone would still accept it. A no-report case needs
/// none, and a provisional report is neither.
#[test]
fn a_second_final_receipt_is_not_success() {
    let mut tally = ReportTally::default();
    assert!(!tally.exactly(&SCENARIOS[0]) && tally.exactly(&SCENARIOS[2]));
    assert!(
        tally
            .observe(Some("report"), br#"{"reportRecovery":{}}"#)
            .is_some()
    );
    assert!(
        !tally.exactly(&SCENARIOS[0]),
        "a provisional report is not final"
    );
    assert!(
        tally
            .observe(Some("report"), br#"{"codingScore":80}"#)
            .is_some()
    );
    assert!(tally.exactly(&SCENARIOS[0]) && !tally.exactly(&SCENARIOS[2]));
    assert!(
        tally
            .observe(Some("report"), br#"{"codingScore":80}"#)
            .is_some()
    );
    assert!(
        tally.matches(&SCENARIOS[0]),
        "distinct bytes alone would accept the resend"
    );
    assert!(
        !tally.exactly(&SCENARIOS[0]),
        "a resend is a second receipt"
    );
    assert_eq!((tally.final_copies, tally.final_ids.len()), (2, 1));
}

fn passing_facts(scenario: &Scenario) -> Facts {
    Facts {
        gate_entered: true,
        agent: AgentEnd::Completed,
        final_copies: usize::from(scenario.report),
        final_distinct: usize::from(scenario.report),
        provisional: 0,
        ack_failures: 0,
        ended_before_end: false,
        agent_after_return: (scenario.name.starts_with('a')).then_some(true),
        end_while_pending: scenario.scripted_end.then_some(true),
        deadline_elapsed_s: Some(725.0),
        absence_elapsed_s: Some(92.0),
        participants_left: Some(Vec::new()),
        readers_unfinished: 0,
        room_gone_after_delete: true,
    }
}

/// Declared before running: the verdict passes the facts each scenario
/// declares, and each single departure from them is named.
#[test]
fn the_verdict_names_each_departure_from_a_scenario() {
    for scenario in &SCENARIOS {
        assert_eq!(
            judge(scenario, &passing_facts(scenario)),
            Vec::<String>::new()
        );
    }

    // Each applies to (a), whose script has the leave, the return and End.
    type Departure = (&'static str, fn(&mut Facts), &'static str);
    let departures: [Departure; 12] = [
        ("gate", |f| f.gate_entered = false, "never entered"),
        ("agent", |f| f.agent = AgentEnd::Aborted, "did not complete"),
        (
            "failed",
            |f| f.agent = AgentEnd::Failed("x".into()),
            "did not complete",
        ),
        (
            "two receipts",
            |f| f.final_copies += 1,
            "final report receipt",
        ),
        (
            "two reports",
            |f| f.final_distinct += 1,
            "final report receipt",
        ),
        ("ack", |f| f.ack_failures = 1, "acknowledgements"),
        (
            "early",
            |f| f.ended_before_end = true,
            "before the scripted End",
        ),
        (
            "left",
            |f| f.participants_left = Some(vec!["x".into()]),
            "participants left",
        ),
        ("room", |f| f.room_gone_after_delete = false, "still listed"),
        ("reader", |f| f.readers_unfinished = 1, "Live reader"),
        (
            "end",
            |f| f.end_while_pending = Some(false),
            "still pending",
        ),
        (
            "return",
            |f| f.agent_after_return = Some(false),
            "after the return",
        ),
    ];
    for (label, change, needle) in departures {
        let scenario = &SCENARIOS[0];
        let mut facts = passing_facts(scenario);
        change(&mut facts);
        let problems = judge(scenario, &facts);
        assert!(
            problems.iter().any(|p| p.contains(needle)),
            "{label}: {problems:?}"
        );
    }
    let report_in_c = Facts {
        final_copies: 1,
        final_distinct: 1,
        ..passing_facts(&SCENARIOS[2])
    };
    assert!(
        !judge(&SCENARIOS[2], &report_in_c).is_empty(),
        "absence leaves with no report"
    );
    let no_report_in_b = Facts {
        final_copies: 0,
        final_distinct: 0,
        ..passing_facts(&SCENARIOS[1])
    };
    assert!(!judge(&SCENARIOS[1], &no_report_in_b).is_empty());
    let early_deadline = Facts {
        deadline_elapsed_s: Some(300.0),
        ..passing_facts(&SCENARIOS[1])
    };
    assert!(judge(&SCENARIOS[1], &early_deadline)[0].contains("hard deadline"));
    let early_absence = Facts {
        absence_elapsed_s: Some(20.0),
        ..passing_facts(&SCENARIOS[2])
    };
    assert!(judge(&SCENARIOS[2], &early_absence)[0].contains("grace"));
    let unobserved = Facts {
        deadline_elapsed_s: None,
        ..passing_facts(&SCENARIOS[1])
    };
    assert!(
        !judge(&SCENARIOS[1], &unobserved).is_empty(),
        "no measurement is not a pass"
    );
}

/// Declared before running: a credential carried in a URL query never reaches
/// a log through an error's text.
#[test]
fn redaction_removes_credentials_carried_in_a_query() {
    for (text, secret) in [
        (
            "wss://x/rtc?access_token=SECRETJWT&protocol=15",
            "SECRETJWT",
        ),
        ("GET /v1?key=AIzaSECRET failed", "AIzaSECRET"),
        ("bad url (https://h/p?foo=1&key=ABC123)", "ABC123"),
        ("a token=XYZ and key=QRS", "XYZ"),
    ] {
        let clean = redact(text);
        assert!(!clean.contains(secret), "{text} -> {clean}");
        assert!(clean.contains("REDACTED"), "{clean}");
    }
    for (text, secret) in [
        ("WSS://x/rtc?Access_Token=UPPERJWT", "UPPERJWT"),
        ("Authorization: Bearer HEADERJWT", "HEADERJWT"),
        ("{\"api_key\": \"JSONKEY\"}", "JSONKEY"),
    ] {
        assert!(!redact(text).contains(secret), "{text}");
    }
    register_secrets(["registered-secret-value".to_owned()]);
    assert!(!redact("oops registered-secret-value here").contains("registered-secret-value"));
    assert_eq!(redact("no credentials here"), "no credentials here");
    assert!(redact("wss://x/rtc?access_token=T&protocol=15").ends_with("&protocol=15"));
}

/// Declared before running: the selector accepts exactly a, b, c and d.
#[test]
fn the_scenario_selector_takes_one_letter() {
    for (letter, name) in [
        ("a", "a_manual_end_with_transient_leave"),
        ("b", "b_server_deadline"),
        ("c", "c_absence_expiry"),
        ("d", "d_end_during_pending_recovery_with_thinking_hold"),
    ] {
        assert_eq!(scenario_for(letter).map(|s| s.name), Some(name));
    }
    for bad in ["", "e", "ab", "a_manual", "A", " a"] {
        assert!(scenario_for(bad).is_none(), "{bad:?}");
    }
}

/// Declared before running: the manifest writer, started as the launcher
/// wrapper starts it, writes for each letter a manifest the controller accepts
/// with its binary pin verified, naming the scenario's own actions. The pinned
/// binary is this test binary.
#[test]
fn the_manifest_writer_produces_what_the_controller_validates() {
    let scratch = Scratch::new("writer");
    let name = format!(
        "{}::live_write_manifest",
        module_path!().split_once("::").unwrap().1
    );
    for scenario in &SCENARIOS {
        let letter = &scenario.name[..1];
        let out = scratch.path(&format!("manifest-{letter}.json"));
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env("CODETRIAL_LIVE_MANIFEST_OUT", &out)
            .env("CODETRIAL_LIVE_SCENARIO", letter)
            .env("CODETRIAL_LIVE_CANDIDATE", std::env::current_exe().unwrap())
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let manifest: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(manifest["actions"], json!(scenario.actions));
        assert_eq!(manifest["run_id"], json!(scenario.name.replace('_', "-")));
        let checked = std::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/interview-run-budget.py"))
            .args(["validate", "--verify-binaries", "--manifest"])
            .arg(&out)
            .output()
            .unwrap();
        assert!(
            checked.status.success(),
            "{}: {}{}",
            scenario.name,
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        );
    }
}

fn private_gate() -> (RecoveryGate, super::super::WriteBarrier) {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    (
        RecoveryGate {
            entered: entered.clone(),
            release: release.clone(),
        },
        super::super::WriteBarrier {
            phase: RECOVERY_GATE_PHASE,
            entered,
            release,
        },
    )
}

/// Declared before running: ending the first Live reader, as the trigger does
/// through the probe's inspection, is a hang-up from the loop's side. The room
/// loop's own Close path opens a replacement on a local socket and reaches the
/// recovery brief write, where the gate is entered while the recovery has not
/// returned; releasing it lets the replacement finish and the old socket's
/// reader is finished. This runs the real recovery driver on a local Gemini
/// socket, not a live one.
#[tokio::test]
async fn ending_the_first_reader_drives_the_close_path_to_the_recovery_gate() {
    if super::isolated("live::ending_the_first_reader_drives_the_close_path_to_the_recovery_gate") {
        return;
    }
    let (listener, endpoint) = super::listener().await;
    let mut rig = super::Rig::new(endpoint).await;
    let (gate, barrier) = private_gate();
    rig.loops.recovery_writes.push_back(barrier);
    let reader = rig
        .gemini
        .inspect_reader_for_test()
        .expect("setup owns its reader");
    assert!(!reader.is_finished());
    reader.abort();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let began = std::time::Instant::now();
    while !reader.is_finished() {
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "the reader did not end"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(2), rig.gemini.next_event())
            .await
            .expect("ending the reader closes the event stream")
            .is_none(),
        "the loop would see a hang-up"
    );
    let server = tokio::spawn(super::recording_fresh_gemini(
        listener,
        vec![super::FreshStep::Ready],
    ));
    let done = std::sync::atomic::AtomicBool::new(false);
    let (flow, entered_while_pending) = tokio::join!(
        async {
            let flow = rig.run().await;
            done.store(true, Ordering::SeqCst);
            flow
        },
        async {
            gate.entered(Duration::from_secs(8)).await.unwrap();
            let pending = !done.load(Ordering::SeqCst);
            gate.release();
            pending
        }
    );
    assert!(entered_while_pending, "the gate is entered mid-recovery");
    assert!(flow.is_continue());
    let _ = rig.old_frames().await;
    assert!(reader.is_finished());
    assert_eq!(server.await.unwrap().len(), 1, "one replacement socket");
}

/// Declared before running: the fixture's setup is local until the join permit.
/// It signs a candidate token under the token endpoint's rules with the
/// candidate identity the agent matches, durationMin 10 and a Two Sum plan,
/// names a room of its own and arms the reader probe, and none of it touches a
/// provider.
#[tokio::test]
async fn the_live_effects_sign_a_candidate_token_locally() {
    use base64::Engine as _;
    let config = crate::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();
    let (gate, _barrier) = private_gate();
    let effects = LiveEffects::new(&SCENARIOS[1], config, None, 10, &gate, None).unwrap();
    assert!(effects.room_name.starts_with("ct-live-b-"));
    assert_eq!(effects.candidate_identity, "candidate-liveb");
    assert_eq!(
        effects.agent_identity,
        crate::runtime::agent_identity(&effects.room_name)
    );
    let claims: Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(effects.token.split('.').nth(1).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(claims["sub"], "candidate-liveb");
    assert_eq!(claims["video"]["room"], json!(effects.room_name));
    assert_eq!(claims["video"]["canPublishData"], true);
    assert_ne!(claims["video"]["agent"], true, "a candidate, not an agent");
    let metadata: Value = serde_json::from_str(claims["metadata"].as_str().unwrap()).unwrap();
    assert_eq!(metadata["candidateIdentity"], "candidate-liveb");
    assert_eq!(metadata["durationMin"], 10);
    assert_eq!(metadata["problemId"], "two-sum");
    assert!(crate::livekit::candidate_identity_matches(
        "candidate-liveb",
        claims["metadata"].as_str().unwrap()
    ));
    assert!(
        super::super::super::take_live_reader_probe().is_some(),
        "the reader probe is armed for the loop that starts"
    );
    assert!(
        effects.link.is_none() && effects.agent.is_none(),
        "nothing is joined"
    );
}

/// Declared before running: the scripted part's budget is the cap less the
/// cleanup minute, so the server-deadline case keeps its whole ending window
/// after the 12 minute deadline, and no case's budget exceeds its cap.
#[test]
fn the_script_budget_keeps_the_ending_window_inside_the_cap() {
    for scenario in &SCENARIOS {
        let budget = script_budget(scenario);
        assert!(budget < Duration::from_secs(scenario.outer_cap_s));
        assert_eq!(
            budget + Duration::from_secs(CLEANUP_S),
            Duration::from_secs(scenario.outer_cap_s)
        );

        // A typical join, the deadline or the scripted observation, then the
        // whole ending window.
        assert!(
            budget.as_secs() >= 30 + scenario.observation_s().min(300) + ENDING_S,
            "{}",
            scenario.name
        );
    }
    assert!(
        script_budget(&SCENARIOS[1]).as_secs() >= 30 + 12 * 60 + ENDING_S,
        "the deadline case reaches its report inside the budget"
    );
}

/// Declared before running: a deadline observation far past the hard deadline
/// plus the ending window is not the deadline ending it.
#[test]
fn a_deadline_far_past_the_hard_deadline_is_not_a_pass() {
    let late = Facts {
        deadline_elapsed_s: Some(720.0 + ENDING_S as f64 + 60.0),
        ..passing_facts(&SCENARIOS[1])
    };
    assert!(judge(&SCENARIOS[1], &late)[0].contains("hard deadline"));
    let edge = Facts {
        deadline_elapsed_s: Some(720.0 + ENDING_S as f64),
        ..passing_facts(&SCENARIOS[1])
    };
    assert!(judge(&SCENARIOS[1], &edge).is_empty());
}

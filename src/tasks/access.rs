//! What the local learner has unlocked and is attempting.
//!
//! Task mode runs on the learner's own machine, for that learner alone, so
//! nothing here defends against them: there are no PIN throttles, grants or
//! deployment quotas. What it keeps is what stops honest mistakes costing
//! anything, such as a retried Start opening a second room.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};

use serde_json::{Value, json};
use tokio::time::Instant;

use super::default_number;
use super::exercise::{Exercise, TaskRecord};
use super::package::{
    Download, LoadedSet, OpenError, SetConfig, download, open_package, site_base,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessError {
    pub code: &'static str,
    pub retry_after: Option<u64>,
}

impl AccessError {
    pub(crate) fn new(code: &'static str) -> Self {
        Self {
            code,
            retry_after: None,
        }
    }
}

/// Sets one process keeps unlocked at once; unlocking another drops the one
/// unlocked longest ago.
const MAX_UNLOCKED_SETS: usize = 8;
/// Start request keys remembered for replay.
const MAX_ADMISSIONS: usize = 256;
/// Bytes of a client's admission request key.
const MAX_REQUEST_KEY_BYTES: usize = 128;

/// One assignment link: the instructor's site and the set version it names.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Assignment {
    pub site: reqwest::Url,
    pub set_id: String,
    pub version: u32,
}

impl Assignment {
    /// Checks a link's parts. A set or version no package can carry is the
    /// link's fault, not the site's, and no download would change it.
    pub fn new(site: &str, set_id: &str, version: u32) -> Result<Self, AccessError> {
        if !super::identifier(set_id) || version == 0 || version > i32::MAX as u32 {
            return Err(AccessError::new("site_invalid"));
        }
        Ok(Self {
            site: site_base(site).map_err(|_| AccessError::new("site_invalid"))?,
            set_id: set_id.to_owned(),
            version,
        })
    }
}

#[derive(Clone)]
pub struct TaskService(Arc<Inner>);

impl PartialEq for TaskService {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for TaskService {}

impl Default for TaskService {
    fn default() -> Self {
        Self::new()
    }
}

struct Inner {
    state: Mutex<State>,
    /// One key derivation at a time: each is deliberately slow.
    unlocking: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct State {
    /// Keyed by account and assignment: two links for versions of one set, or
    /// for sets two sites happen to name alike, are two sets and neither
    /// replaces the other.
    unlocked: HashMap<(i64, Assignment), Unlocked>,
    unlock_order: VecDeque<(i64, Assignment)>,
    next_claim_id: u64,
    /// The one attempt this process runs, whoever's it is: one CodeTrial
    /// serves one learner, and keeps one result.
    active: Option<ActiveAttempt>,
    ordinals: HashMap<(i64, String), u64>,
    admissions: HashMap<(i64, String), PendingAdmission>,
    admission_order: VecDeque<(i64, String)>,
    result: Option<StoredResult>,
}

struct ActiveAttempt {
    owner_id: i64,
    claim_id: u64,
    /// The room its admission answered with, once it has.
    room: Option<String>,
}

impl State {
    /// Holds `unlocked` as the most recently opened set, letting the least
    /// recent go past the cap. One is added at a time, so one is all that can
    /// be over.
    fn remember(&mut self, key: (i64, Assignment), unlocked: Unlocked) {
        self.unlock_order.retain(|held| held != &key);
        self.unlock_order.push_back(key.clone());
        self.unlocked.insert(key, unlocked);
        if self.unlock_order.len() > MAX_UNLOCKED_SETS
            && let Some(oldest) = self.unlock_order.pop_front()
        {
            self.unlocked.remove(&oldest);
        }
    }

    /// Drops a kept result once its time is up, at whatever request comes
    /// next, so an expired review and its code are not held for the life of
    /// the process.
    fn purge(&mut self, now: u64) {
        if self
            .result
            .as_ref()
            .is_some_and(|result| result.expires_at <= now)
        {
            self.result = None;
        }
    }
}

struct Unlocked {
    set: Arc<LoadedSet>,
    /// The instructor site's clock at download, and when that was, so a
    /// close date is judged by the site rather than the learner's clock.
    site_clock: Option<(u64, Instant)>,
}

impl Unlocked {
    fn now(&self, local_now: u64) -> u64 {
        self.site_clock
            .map_or(local_now, |(at, since)| at + since.elapsed().as_secs())
    }
}

struct StoredResult {
    owner_id: i64,
    room: String,
    expires_at: u64,
    value: Value,
}

/// What the learner did before Start, carried into the attempt's result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Preparation {
    pub rules_acknowledged_at: String,
    pub calibration: Value,
}

#[derive(Clone)]
pub struct PinnedTask {
    /// The set's rules, not the set: an attempt reads its own record, which
    /// `exercise` holds, and keeping the whole decoded set alive for the
    /// room's lifetime would outlast its eviction from the unlocked sets.
    pub config: Arc<SetConfig>,
    pub exercise: Exercise,
    pub owner_id: i64,
    pub github_login: String,
    pub github_id: u64,
    pub session_ordinal: u64,
    pub duration_min: u32,
    pub hint_limit: usize,
    pub claim_id: u64,
    pub preparation: Preparation,
    admission_lease: Arc<AdmissionLease>,
}

impl PinnedTask {
    /// Keeps the attempt's result, final code included, for the learner's
    /// page to collect after it lost its room or was reloaded. Only the latest
    /// result is kept.
    pub fn retain_result(&self, room: &str, result: Value, now: u64) {
        let Some(inner) = self.admission_lease.inner.upgrade() else {
            return;
        };
        let value = if result.to_string().len() <= default_number("reviewBytes") as usize {
            result
        } else {
            super::report::unavailable_for_task(self, room, &result)
        };
        inner.state.lock().unwrap_or_else(|e| e.into_inner()).result = Some(StoredResult {
            owner_id: self.owner_id,
            room: room.to_owned(),
            expires_at: now.saturating_add(default_number("reviewRetentionSeconds")),
            value,
        });
    }
}

struct PendingAdmission {
    claim_id: u64,
    /// Assignment and task: a replayed request key must name the same.
    assignment: (Assignment, String),
    expires_at: u64,
    response: Option<Value>,
}

struct AdmissionLease {
    inner: Weak<Inner>,
    claim_id: u64,
}

impl Drop for AdmissionLease {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if state
                .active
                .as_ref()
                .is_some_and(|active| active.claim_id == self.claim_id)
            {
                state.active = None;
            }
        }
    }
}

pub enum Admission {
    New(PinnedTask),
    Replayed(Value),
}

/// What the result endpoint can say about one room.
#[derive(Debug, PartialEq)]
pub enum ResultLookup {
    Ready(Value),
    /// The account still has an attempt being finalized.
    Pending,
    /// No result exists for that room, and none is coming.
    Unavailable,
}

/// The hint rungs a task may use: the package's maximum, lowered by the set.
fn hint_limit(set: &LoadedSet, record: &TaskRecord) -> usize {
    record
        .sidecar()
        .max_hint_rungs
        .min(set.config.rule("maxHintRungs") as usize)
}

impl TaskService {
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            state: Mutex::default(),
            unlocking: tokio::sync::Mutex::new(()),
        }))
    }

    /// Downloads one set version from the instructor's site and opens it with
    /// the learner's PIN.
    pub async fn unlock(
        &self,
        owner_id: i64,
        assignment: &Assignment,
        pin: &str,
        now: u64,
    ) -> Result<Value, AccessError> {
        if !super::package::valid_pin(pin) {
            return Err(AccessError::new("invalid_pin"));
        }
        let downloaded = download(assignment)
            .await
            .map_err(|_| AccessError::new("package_unavailable"))?;
        self.open(owner_id, assignment, downloaded, pin, now).await
    }

    /// `unlock` after the download: derives the key off the async workers, one
    /// derivation at a time, then keeps the decoded set for this account.
    /// Public so a test can open a package it already holds without a site.
    pub async fn open(
        &self,
        owner_id: i64,
        assignment: &Assignment,
        downloaded: Download,
        pin: &str,
        now: u64,
    ) -> Result<Value, AccessError> {
        let _one_at_a_time = self.0.unlocking.lock().await;
        let (set_id, version) = (assignment.set_id.as_str(), assignment.version);
        let (id, pin) = (set_id.to_owned(), pin.to_owned());
        let Download {
            manifest,
            ciphertext,
            site_time,
        } = downloaded;
        let opened = tokio::task::spawn_blocking(move || {
            open_package(&id, version, &manifest, ciphertext, &pin)
        })
        .await
        .map_err(|_| AccessError::new("package_invalid"))?;
        let set = match opened {
            Ok(set) => Arc::new(set),
            Err(OpenError::WrongPin) => return Err(AccessError::new("invalid_pin")),
            Err(OpenError::Invalid(_)) => return Err(AccessError::new("package_invalid")),
        };
        let unlocked = Unlocked {
            set: set.clone(),
            site_clock: site_time,
        };
        if set.config.closed(unlocked.now(now)) {
            return Err(AccessError::new("set_closed"));
        }
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remember((owner_id, assignment.clone()), unlocked);
        Ok(json!({"setId": set_id, "version": version,
            "tasks": set.records.iter().map(|(id, record)| json!({"id": id, "title": Exercise(record.clone()).title()})).collect::<Vec<_>>()}))
    }

    /// The public projection of one task in an unlocked set.
    pub fn load_task(
        &self,
        owner_id: i64,
        assignment: &Assignment,
        task_id: &str,
    ) -> Result<Value, AccessError> {
        // The set is shared; the lock is held only to find it.
        let set = self
            .0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .unlocked
            .get(&(owner_id, assignment.clone()))
            .ok_or_else(|| AccessError::new("unlock_required"))?
            .set
            .clone();
        let record = set
            .records
            .get(task_id)
            .ok_or_else(|| AccessError::new("task_not_found"))?;
        let exercise = Exercise(record.clone());
        let projection = exercise.live_projection();
        let config = &set.config;
        Ok(
            json!({"setId": assignment.set_id, "version": config.version, "taskId": task_id,
            "title": exercise.title(), "brief": projection["brief"], "contract": projection["contract"],
            "starterCode": {"python": exercise.starter("python")}, "publicCases": record.judge(),
            "completionTargets": record.sidecar().completion_targets, "understandingChecks": record.sidecar().understanding_checks,
            "rules": config.rules_json(),
            "setupTimeoutSeconds": default_number("pendingAdmissionSeconds"),
            "reviewDeliverySeconds": default_number("reviewDeliverySeconds")}),
        )
    }

    /// Starts an attempt, or replays the one a retried request key already
    /// started.
    #[allow(clippy::too_many_arguments)]
    pub fn begin_admission(
        &self,
        owner_id: i64,
        github: (&str, u64),
        assignment: &Assignment,
        task_id: &str,
        request_key: &str,
        preparation: Preparation,
        now: u64,
    ) -> Result<Admission, AccessError> {
        if !super::bounded_text(request_key, MAX_REQUEST_KEY_BYTES) {
            return Err(AccessError::new("client_override"));
        }
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.purge(now);
        let key = (owner_id, request_key.to_owned());
        let named = (assignment.clone(), task_id.to_owned());
        if let Some(admission) = state.admissions.get(&key) {
            if admission.assignment != named {
                return Err(AccessError::new("client_override"));
            }
            if admission.expires_at <= now
                || state
                    .active
                    .as_ref()
                    .is_none_or(|active| active.claim_id != admission.claim_id)
            {
                return Err(AccessError::new("request_key_expired"));
            }
            return admission
                .response
                .clone()
                .map(Admission::Replayed)
                .ok_or(AccessError {
                    code: "setup_pending",
                    retry_after: Some(1),
                });
        }
        if state.active.is_some() {
            return Err(AccessError::new("active_task_session"));
        }
        let unlocked = state
            .unlocked
            .get(&(owner_id, assignment.clone()))
            .ok_or_else(|| AccessError::new("unlock_required"))?;
        if unlocked.set.config.closed(unlocked.now(now)) {
            return Err(AccessError::new("set_closed"));
        }
        let set = unlocked.set.clone();
        let record = set
            .records
            .get(task_id)
            .ok_or_else(|| AccessError::new("task_not_found"))?
            .clone();
        let claim_id = state.next_claim_id + 1;
        state.next_claim_id = claim_id;
        let ordinal = state
            .ordinals
            .entry((owner_id, assignment.set_id.clone()))
            .or_default();
        *ordinal += 1;
        let session_ordinal = *ordinal;
        state.active = Some(ActiveAttempt {
            owner_id,
            claim_id,
            room: None,
        });
        state.admissions.insert(
            key.clone(),
            PendingAdmission {
                claim_id,
                assignment: named,
                expires_at: now.saturating_add(default_number("pendingAdmissionSeconds")),
                response: None,
            },
        );
        state.admission_order.push_back(key);
        // One is added at a time, so one is all that can be over.
        if state.admission_order.len() > MAX_ADMISSIONS
            && let Some(oldest) = state.admission_order.pop_front()
        {
            state.admissions.remove(&oldest);
        }
        Ok(Admission::New(PinnedTask {
            hint_limit: hint_limit(&set, &record),
            duration_min: set.config.rule("durationMin") as u32,
            exercise: Exercise(record),
            config: Arc::new(set.config.clone()),
            owner_id,
            github_login: github.0.to_owned(),
            github_id: github.1,
            session_ordinal,
            claim_id,
            preparation,
            admission_lease: Arc::new(AdmissionLease {
                inner: Arc::downgrade(&self.0),
                claim_id,
            }),
        }))
    }

    /// Records the answer a replayed Start gets, or forgets a Start that
    /// failed before it dispatched anything.
    pub fn finish_admission(
        &self,
        owner_id: i64,
        request_key: &str,
        claim_id: u64,
        response: Option<Value>,
    ) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        let key = (owner_id, request_key.to_owned());
        if state
            .admissions
            .get(&key)
            .is_none_or(|admission| admission.claim_id != claim_id)
        {
            return;
        }
        match response {
            Some(response) => {
                if let Some(active) = state
                    .active
                    .as_mut()
                    .filter(|active| active.claim_id == claim_id)
                {
                    active.room = response["roomName"].as_str().map(str::to_owned);
                }
                if let Some(admission) = state.admissions.get_mut(&key) {
                    admission.response = Some(response);
                }
            }
            None => {
                state.admissions.remove(&key);
                state.admission_order.retain(|held| held != &key);
            }
        }
    }

    /// The result of the account's attempt in `room`.
    pub fn result(&self, owner_id: i64, room: &str, now: u64) -> ResultLookup {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        state.purge(now);
        match &state.result {
            Some(result) if result.owner_id == owner_id && result.room == room => {
                ResultLookup::Ready(result.value.clone())
            }

            // Pending only for the attempt still running in that very room: a
            // stale or wrong room id gets no result now or later.
            _ if state.active.as_ref().is_some_and(|active| {
                active.owner_id == owner_id && active.room.as_deref() == Some(room)
            }) =>
            {
                ResultLookup::Pending
            }
            _ => ResultLookup::Unavailable,
        }
    }
}

/// Puts an already decoded set in place, as `open` would, so tests do not pay
/// the key derivation for every case.
#[cfg(test)]
impl TaskService {
    pub(crate) fn insert_unlocked(&self, owner_id: i64, site: &str, set: Arc<LoadedSet>) {
        let mut state = self.0.state.lock().unwrap();
        let assignment = Assignment::new(site, &set.config.id, set.config.version).unwrap();
        state.remember(
            (owner_id, assignment),
            Unlocked {
                set,
                site_clock: None,
            },
        );
    }
}

#[cfg(test)]
#[path = "../../tests/unit/tasks/access.rs"]
mod tests;

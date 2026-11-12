//! Running interviewers for the rooms this process named.
//!
//! Separate from `main` because none of it is startup: the cap, the slot
//! accounting and the spawn all happen on the request path, for the length of
//! an interview, long after the binary finished deciding what it is. Here it
//! also gets tests that do not need a process.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::config::{AgentConfig, Provider};
use crate::web::{AgentJob, DispatchRefusal, NoShowConsent, RoomDispatcher};

const MAX_CONCURRENT_GUEST_INTERVIEWS: usize = 2;

/// Starts one signed-in account may hold that no candidate has joined. Each
/// reserves a slot and spawns an agent that joins LiveKit, a signed-in session
/// is free to mint, and the rate limit counts requests, not the rooms they
/// leave
/// waiting, so the count is bounded here. A page that is reloaded, or opened in
/// a second tab, holds more than one for a moment, so it is not one.
const MAX_WAITING_STARTS_PER_ACCOUNT: usize = 4;

/// Interview admission state protected by a single mutex.
#[derive(Default)]
pub struct DispatchState {
    live: HashMap<String, bool>,
    guest_live: HashSet<String>,
    /// Whose each reserved room is and whether they have arrived, so a page
    /// that gives up its start can end the wait it began and nobody else's.
    starts: HashMap<String, Start>,
    /// Set once by `close_admission`: no new room is admitted after it, under
    /// the same lock that decides every admission and `is_idle`.
    closed: bool,
}

struct Start {
    /// The identity in the token this start handed out. `None` for a start
    /// made without one, such as a task attempt, which no release can match.
    candidate: Option<String>,
    /// The account that made it, where it was made by one.
    owner: Option<i64>,
    phase: StartPhase,
    released: Arc<tokio::sync::Notify>,
}

/// Moves only forward, and only under the dispatch lock, so a release and a
/// join that race each other agree on which of them won.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartPhase {
    Waiting,
    /// A candidate was accepted. A reconnect never takes it back to waiting.
    Joined,
    Released,
}

/// Runs the interviewer for rooms this process just named, in this process.
///
/// The alternative is a LiveKit agent worker registered against the project,
/// which is the shape LiveKit intends and which survives this process
/// restarting. This is the smaller thing that makes production work: the room
/// name is invented in `/api/token` and never leaves this process, so the
/// process that invented it is the one that can act on it.
pub struct LocalDispatcher {
    /// Everything an interview needs except which LiveKit project it is in:
    /// that arrives with the room, from the same lookup that minted the token.
    pub config: AgentConfig,
    pub runtime: tokio::runtime::Handle,

    // A poisoned lock is recovered: the maps are written only after the last
    // call that can panic (the `expect` in `claim_join` fires before it
    // writes), and `Slot::drop` takes this lock, possibly while a task unwinds,
    // where a panic would abort the process and leak the room's capacity.
    /// Interview and guest-capacity state protected by one mutex.
    pub state: Arc<Mutex<DispatchState>>,
    /// From `config::max_concurrent_interviews`, so the ceiling an operator set
    /// is the ceiling this enforces.
    pub max_concurrent: usize,
}

/// Test-only observer of the assignment boundary: told each room the dispatcher
/// admits as new, after admission and before its task is spawned, so a fixture
/// can name the room it will see without choosing the name. It changes no
/// dispatch behavior.
#[cfg(test)]
pub(crate) type AssignmentHook = Arc<dyn Fn(&str) + Send + Sync>;
#[cfg(test)]
static ASSIGNED: Mutex<Option<AssignmentHook>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn set_assignment_hook(hook: Option<AssignmentHook>) {
    *ASSIGNED.lock().unwrap_or_else(|error| error.into_inner()) = hook;
}

/// Whether `hook` is the one installed. Compares the allocations, not the
/// vtables.
#[cfg(test)]
pub(crate) fn assignment_hook_is(hook: &AssignmentHook) -> bool {
    ASSIGNED
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .is_some_and(|now| std::ptr::addr_eq(Arc::as_ptr(now), Arc::as_ptr(hook)))
}

/// Removes `hook` only if it is still the installed one, so a teardown never
/// clears a successor's.
#[cfg(test)]
pub(crate) fn clear_assignment_hook(hook: &AssignmentHook) {
    let mut slot = ASSIGNED.lock().unwrap_or_else(|error| error.into_inner());
    if slot
        .as_ref()
        .is_some_and(|now| std::ptr::addr_eq(Arc::as_ptr(now), Arc::as_ptr(hook)))
    {
        *slot = None;
    }
}

#[cfg(test)]
pub(crate) fn notify_assigned(room: &str) {
    let hook = ASSIGNED
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    if let Some(hook) = hook {
        hook(room);
    }
}

impl RoomDispatcher for LocalDispatcher {
    /// A room stays in `live` from admission until its task ends, so one whose
    /// report is still being written counts too.
    fn is_idle(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.live.is_empty()
    }

    fn supports_guest_admission(&self) -> bool {
        true
    }

    fn close_admission(&self) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .closed = true;
    }

    fn candidate_video(&self) -> Option<bool> {
        Some(self.config.gemini_candidate_video_enabled)
    }

    fn ensure_agent(
        &self,
        room_name: &str,
        provider: &Provider,
        job: AgentJob,
    ) -> Result<(), DispatchRefusal> {
        self.ensure_agent_with_kind(room_name, provider, None, None, None, job)
    }

    fn ensure_agent_with_no_show_release(
        &self,
        room_name: &str,
        provider: &Provider,
        candidate: &str,
        no_show: Option<NoShowConsent>,
        owner: Option<i64>,
    ) -> Result<(), DispatchRefusal> {
        let start = Some((candidate, false));
        self.ensure_agent_with_kind(
            room_name,
            provider,
            start,
            no_show,
            owner,
            AgentJob::Interview,
        )
    }

    fn ensure_guest_agent(
        &self,
        room_name: &str,
        provider: &Provider,
        candidate: &str,
    ) -> Result<(), DispatchRefusal> {
        let start = Some((candidate, true));
        self.ensure_agent_with_kind(room_name, provider, start, None, None, AgentJob::Interview)
    }

    fn release_start(&self, room_name: &str, candidate: &str) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let Some(start) = state.starts.get_mut(room_name) else {
            return false;
        };
        if start.phase != StartPhase::Waiting || start.candidate.as_deref() != Some(candidate) {
            return false;
        }
        start.phase = StartPhase::Released;

        // `notify_one` keeps a permit when nobody is waiting yet, so a release
        // that lands before the room task reaches its wait is not lost.
        start.released.notify_one();
        true
    }
}

impl LocalDispatcher {
    /// `start` is the candidate identity a token was minted for, and whether
    /// that caller is a guest. `None` is a signed-in start no page can release.
    fn ensure_agent_with_kind(
        &self,
        room_name: &str,
        provider: &Provider,
        start: Option<(&str, bool)>,
        no_show: Option<NoShowConsent>,
        owner: Option<i64>,
        job: AgentJob,
    ) -> Result<(), DispatchRefusal> {
        let (candidate, guest) = start.map_or((None, false), |(candidate, guest)| {
            (Some(candidate.to_string()), guest)
        });
        let slot = match self.reserve_for(room_name, guest, candidate, owner) {
            Reservation::Existing => return Ok(()),
            Reservation::Full { limit, guest_cap } => {
                eprintln!(
                    "codetrial dispatch_refused room={room_name} reason=at_capacity \
                     limit={limit} guest={guest} guest_cap={guest_cap}"
                );
                return Err(DispatchRefusal::AtCapacity);
            }
            Reservation::GuestRoomInUse => {
                eprintln!(
                    "codetrial dispatch_refused room={room_name} \
                     reason=guest_room_in_use guest=true"
                );
                return Err(DispatchRefusal::AtCapacity);
            }
            Reservation::TooManyWaiting => {
                eprintln!("codetrial dispatch_refused room={room_name} reason=too_many_waiting");
                return Err(DispatchRefusal::TooManyWaiting);
            }
            Reservation::Finalizing => {
                eprintln!("codetrial dispatch_refused room={room_name} reason=finalizing");
                return Err(DispatchRefusal::Finalizing);
            }
            Reservation::Closed => {
                eprintln!("codetrial dispatch_refused room={room_name} reason=closed");
                return Err(DispatchRefusal::Stopping);
            }
            Reservation::New(slot) => slot,
        };
        let config = agent_config_for(&self.config, provider);
        #[cfg(test)]
        notify_assigned(&slot.room_name);

        // Spawned, not awaited: this runs on the request path, and the task
        // outlives the response by the length of the interview or attempt.
        self.runtime.spawn(async move {
            eprintln!("codetrial dispatch room={}", slot.room_name);
            match job {
                AgentJob::Interview => {
                    let outcome = contain_panic(
                        crate::livekit::run_room_with_slot(
                            &config,
                            &slot.room_name,
                            crate::web::current_epoch_seconds(),
                            Some(&slot),
                        ),
                        || slot.has_joined(),
                    )
                    .await;
                    settle_interview(outcome, no_show, slot).await;
                }
                AgentJob::Task { candidate, task } => {
                    if let Err(error) = crate::livekit::tasks::run(
                        &config,
                        &slot.room_name,
                        &candidate,
                        task,
                        &slot,
                    )
                    .await
                    {
                        eprintln!("codetrial agent_failed room={}: {error}", slot.room_name);
                    }
                }
            }
        });
        Ok(())
    }
}

/// Runs a room to its outcome, and turns a panic in it into one. The task is
/// detached, so a panic would otherwise skip the settling below and leave the
/// consent claim held, with no line in the log.
///
/// A panic before a candidate joined is a start that failed before one, which
/// returns consent as a no-show does. After the join the interview had begun,
/// and its consent is not given back as if nobody came: it is an error, logged
/// and nothing more. `joined` is asked only after the panic.
async fn contain_panic(
    room: impl std::future::Future<
        Output = Result<crate::livekit::RoomRunOutcome, Box<dyn std::error::Error + Send + Sync>>,
    >,
    joined: impl FnOnce() -> bool,
) -> Result<crate::livekit::RoomRunOutcome, Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::FutureExt as _;
    match std::panic::AssertUnwindSafe(room).catch_unwind().await {
        Ok(outcome) => outcome,
        Err(_) if joined() => Err("the room task panicked after the candidate joined".into()),
        Err(_) => Ok(crate::livekit::RoomRunOutcome::FailedBeforeCandidate(
            "the room task panicked".into(),
        )),
    }
}

/// What a room's task does once its interview has ended, however it ended.
///
/// Takes the slot so the order is the signature's and not a reader's to
/// infer: consent goes back first, and the capacity and guest slot only when
/// the slot drops at the end. A released start arrives here as `NoCandidate`,
/// the same way a no-show does.
async fn settle_interview(
    outcome: Result<crate::livekit::RoomRunOutcome, Box<dyn std::error::Error + Send + Sync>>,
    no_show: Option<NoShowConsent>,
    slot: Slot,
) {
    if outcome.as_ref().is_ok_and(no_show_release_required) {
        release_no_show(no_show, &slot.room_name).await;
    }
    match outcome {
        Ok(crate::livekit::RoomRunOutcome::NoCandidate) => {}
        Ok(crate::livekit::RoomRunOutcome::FailedBeforeCandidate(error)) => {
            eprintln!("codetrial agent_failed room={}: {error}", slot.room_name);
        }
        Ok(crate::livekit::RoomRunOutcome::Completed) => {}
        Err(error) => {
            eprintln!("codetrial agent_failed room={}: {error}", slot.room_name);
        }
    }
}

fn no_show_release_required(outcome: &crate::livekit::RoomRunOutcome) -> bool {
    matches!(
        outcome,
        crate::livekit::RoomRunOutcome::NoCandidate
            | crate::livekit::RoomRunOutcome::FailedBeforeCandidate(_)
    )
}

async fn release_no_show(no_show: Option<NoShowConsent>, room_name: &str) -> bool {
    let Some(no_show) = no_show else {
        return false;
    };
    match no_show.release().await {
        Ok(true) => true,
        Ok(false) => {
            eprintln!(
                "codetrial consent_release_skipped room={room_name} \
                 reason=claim_changed_pending_or_recording_started"
            );
            false
        }
        Err(error) => {
            eprintln!(
                "could not release interview consent after a no-show in room={room_name}: {error}"
            );
            false
        }
    }
}

enum Reservation {
    Existing,
    New(Slot),
    Full {
        limit: usize,
        guest_cap: bool,
    },
    /// The account already holds as many unjoined starts as it may.
    TooManyWaiting,
    GuestRoomInUse,
    Finalizing,
    Closed,
}

impl LocalDispatcher {
    #[cfg(test)]
    fn reserve_with_kind(&self, room_name: &str, guest: bool) -> Reservation {
        self.reserve_for(room_name, guest, None, None)
    }

    /// `Existing` records nothing about `candidate`: the start already running
    /// in that room belongs to whoever began it, and a second token for the
    /// same room must not be able to end it.
    fn reserve_for(
        &self,
        room_name: &str,
        guest: bool,
        candidate: Option<String>,
        owner: Option<i64>,
    ) -> Reservation {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());

        // A room already in use cannot be reused for another interview.
        if let Some(assessing) = state.live.get(room_name) {
            // Released, the start still holds the room until its agent has left
            // and its consent is back. Handing a new token for it the room as
            // `Existing` would put that candidate in a room nobody staffs, so
            // it waits like a room finishing its report.
            if state
                .starts
                .get(room_name)
                .is_some_and(|start| start.phase == StartPhase::Released)
            {
                return Reservation::Finalizing;
            }
            if guest || state.guest_live.contains(room_name) {
                return Reservation::GuestRoomInUse;
            }

            return if *assessing {
                Reservation::Existing
            } else {
                Reservation::Finalizing
            };
        }

        if state.closed {
            return Reservation::Closed;
        }

        if state.live.len() >= self.max_concurrent {
            return Reservation::Full {
                limit: self.max_concurrent,
                guest_cap: false,
            };
        }

        if guest && state.guest_live.len() >= MAX_CONCURRENT_GUEST_INTERVIEWS {
            return Reservation::Full {
                limit: MAX_CONCURRENT_GUEST_INTERVIEWS,
                guest_cap: true,
            };
        }

        if owner.is_some_and(|owner| {
            let waiting = state
                .starts
                .values()
                .filter(|start| start.owner == Some(owner) && start.phase == StartPhase::Waiting)
                .count();
            waiting >= MAX_WAITING_STARTS_PER_ACCOUNT
        }) {
            return Reservation::TooManyWaiting;
        }

        state.live.insert(room_name.to_string(), true);
        if guest {
            state.guest_live.insert(room_name.to_string());
        }
        let released = Arc::new(tokio::sync::Notify::new());
        state.starts.insert(
            room_name.to_string(),
            Start {
                candidate,
                owner,
                phase: StartPhase::Waiting,
                released: Arc::clone(&released),
            },
        );

        Reservation::New(Slot {
            state: Arc::clone(&self.state),
            guest,
            room_name: room_name.to_string(),
            released,
        })
    }
}

/// One interview's claim on this process's capacity, held for as long as the
/// task that owns it.
pub struct Slot {
    state: Arc<Mutex<DispatchState>>,
    pub guest: bool,
    pub room_name: String,
    released: Arc<tokio::sync::Notify>,
}

impl Slot {
    /// Resolves once the page that began this start has given it up.
    pub(crate) async fn released(&self) {
        self.released.notified().await;
    }

    /// Whether the start has already been given up, read without taking the
    /// release [`Slot::released`] waits on.
    pub(crate) fn is_released(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state
            .starts
            .get(&self.room_name)
            .is_some_and(|start| start.phase == StartPhase::Released)
    }

    /// Whether a candidate was accepted into this start.
    fn has_joined(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state
            .starts
            .get(&self.room_name)
            .is_some_and(|start| start.phase == StartPhase::Joined)
    }

    /// Records that a candidate was accepted, unless the start was released
    /// first. `false` means the room is to be left as a no-show.
    pub(crate) fn claim_join(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());

        // Every slot is made beside its start, and only its own drop removes
        // it, so a slot still being asked always finds one.
        let start = state
            .starts
            .get_mut(&self.room_name)
            .expect("a live slot keeps its start");
        if start.phase == StartPhase::Released {
            return false;
        }
        start.phase = StartPhase::Joined;
        true
    }

    pub(crate) fn assessment_finished(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());

        if let Some(assessing) = state.live.get_mut(&self.room_name) {
            *assessing = false;
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());

        state.live.remove(&self.room_name);
        state.starts.remove(&self.room_name);
        if self.guest {
            state.guest_live.remove(&self.room_name);
        }
    }
}
/// The credential half of provider selection, without the pool lookup, for the
/// caller that was already handed the provider.
pub fn agent_config_for(base: &AgentConfig, provider: &Provider) -> AgentConfig {
    let mut config = base.clone();
    config.livekit_url = provider.url.clone();
    config.livekit_api_key = provider.api_key.clone();
    config.livekit_api_secret = provider.api_secret.clone();

    // A provider that brings any Google key of its own replaces the global
    // ones, so its rooms never bill another project.
    if !provider.google_api_keys.is_empty() {
        config.google_api_keys = provider.google_api_keys.clone();
    }
    config
}

#[cfg(test)]
#[path = "../tests/unit/dispatch.rs"]
mod tests;

//! Running interviewers for the rooms this process named.
//!
//! Separate from `main` because none of it is startup: the cap, the slot
//! accounting and the spawn all happen on the request path, for the length of
//! an interview, long after the binary finished deciding what it is. Here it
//! also gets tests that do not need a process.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::config::{AgentConfig, Provider};
use crate::web::{DispatchRefusal, NoShowConsent, RoomDispatcher};

const MAX_CONCURRENT_GUEST_INTERVIEWS: usize = 2;

/// Interview admission state protected by a single mutex.
#[derive(Default)]
pub struct DispatchState {
    live: HashMap<String, bool>,
    guest_live: HashSet<String>,
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

    /// Interview and guest-capacity state protected by one mutex.
    pub state: Arc<Mutex<DispatchState>>,
    /// From `config::max_concurrent_interviews`, so the ceiling an operator set
    /// is the ceiling this enforces.
    pub max_concurrent: usize,
}

impl RoomDispatcher for LocalDispatcher {
    fn supports_guest_admission(&self) -> bool {
        true
    }

    fn candidate_video(&self) -> Option<bool> {
        Some(self.config.gemini_candidate_video_enabled)
    }

    fn ensure_agent(&self, room_name: &str, provider: &Provider) -> Result<(), DispatchRefusal> {
        self.ensure_agent_with_kind(room_name, provider, false, None)
    }

    fn ensure_agent_with_no_show_release(
        &self,
        room_name: &str,
        provider: &Provider,
        no_show: Option<NoShowConsent>,
    ) -> Result<(), DispatchRefusal> {
        self.ensure_agent_with_kind(room_name, provider, false, no_show)
    }

    fn ensure_guest_agent(
        &self,
        room_name: &str,
        provider: &Provider,
    ) -> Result<(), DispatchRefusal> {
        self.ensure_agent_with_kind(room_name, provider, true, None)
    }
}

impl LocalDispatcher {
    fn ensure_agent_with_kind(
        &self,
        room_name: &str,
        provider: &Provider,
        guest: bool,
        no_show: Option<NoShowConsent>,
    ) -> Result<(), DispatchRefusal> {
        let slot = match self.reserve_with_kind(room_name, guest) {
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
            Reservation::Finalizing => {
                eprintln!("codetrial dispatch_refused room={room_name} reason=finalizing");
                return Err(DispatchRefusal::Finalizing);
            }
            Reservation::New(slot) => slot,
        };
        let config = agent_config_for(&self.config, provider);

        // Spawned, not awaited: this runs on the request path, and the task
        // outlives the response by the length of the interview.
        self.runtime.spawn(async move {
            eprintln!("codetrial dispatch room={}", slot.room_name);
            let outcome = crate::livekit::run_room_with_slot(
                &config,
                &slot.room_name,
                crate::web::current_epoch_seconds(),
                Some(&slot),
            )
            .await;
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
        });
        Ok(())
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
    Full { limit: usize, guest_cap: bool },
    GuestRoomInUse,
    Finalizing,
}

impl LocalDispatcher {
    fn reserve_with_kind(&self, room_name: &str, guest: bool) -> Reservation {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());

        // A room already in use cannot be reused for another interview.
        if let Some(assessing) = state.live.get(room_name) {
            if guest || state.guest_live.contains(room_name) {
                return Reservation::GuestRoomInUse;
            }

            return if *assessing {
                Reservation::Existing
            } else {
                Reservation::Finalizing
            };
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

        state.live.insert(room_name.to_string(), true);
        if guest {
            state.guest_live.insert(room_name.to_string());
        }

        Reservation::New(Slot {
            state: Arc::clone(&self.state),
            guest,
            room_name: room_name.to_string(),
        })
    }
}

/// One interview's claim on this process's capacity, held for as long as the
/// task that owns it.
pub struct Slot {
    state: Arc<Mutex<DispatchState>>,
    pub guest: bool,
    pub room_name: String,
}

impl Slot {
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

//! Running interviewers for the rooms this process named.
//!
//! Separate from `main` because none of it is startup: the cap, the slot
//! accounting and the spawn all happen on the request path, for the length of
//! an interview, long after the binary finished deciding what it is. Here it
//! also gets tests that do not need a process.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::{AgentConfig, Provider};
use crate::web::{AgentJob, DispatchRefusal, RoomDispatcher};

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

    // True while assessing; false while finalizing the report. Both hold
    // capacity.
    pub live: Arc<Mutex<HashMap<String, bool>>>,
    /// From `config::max_concurrent_interviews`, so the ceiling an operator set
    /// is the ceiling this enforces.
    pub max_concurrent: usize,
}

impl RoomDispatcher for LocalDispatcher {
    fn ensure_agent(
        &self,
        room_name: &str,
        provider: &Provider,
        job: AgentJob,
    ) -> Result<(), DispatchRefusal> {
        let slot = match self.reserve(room_name) {
            Reservation::Existing => return Ok(()),
            Reservation::Full => {
                eprintln!(
                    "codetrial dispatch_refused room={room_name} reason=at_capacity limit={}",
                    self.max_concurrent
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
        // outlives the response by the length of the interview or attempt.
        self.runtime.spawn(async move {
            eprintln!("codetrial dispatch room={}", slot.room_name);
            let outcome = match job {
                AgentJob::Interview => {
                    crate::livekit::run_room_with_slot(
                        &config,
                        &slot.room_name,
                        crate::web::current_epoch_seconds(),
                        Some(&slot),
                    )
                    .await
                }
                AgentJob::Task { candidate, task } => {
                    crate::livekit::tasks::run(&config, &slot.room_name, &candidate, task, &slot)
                        .await
                }
            };
            if let Err(error) = outcome {
                eprintln!("codetrial agent_failed room={}: {error}", slot.room_name);
            }
        });
        Ok(())
    }
}

enum Reservation {
    Existing,
    New(Slot),
    Full,
    Finalizing,
}

impl LocalDispatcher {
    fn reserve(&self, room_name: &str) -> Reservation {
        let mut live = self.live.lock().unwrap_or_else(|error| error.into_inner());

        // A reload mints a token for the same fixed room in local mode, and two
        // agents in one room evict each other.
        if let Some(assessing) = live.get(room_name) {
            // A fixed local room cannot start a new interview while its old
            // agent is finalizing. Reusing it would mint a token for an agent
            // that only accepts the original candidate's report retry.
            return if *assessing {
                Reservation::Existing
            } else {
                Reservation::Finalizing
            };
        }
        if live.len() >= self.max_concurrent {
            return Reservation::Full;
        }
        live.insert(room_name.to_string(), true);
        Reservation::New(Slot {
            live: Arc::clone(&self.live),
            room_name: room_name.to_string(),
        })
    }
}

/// One interview's claim on this process's capacity, held for as long as the
/// task that owns it.
pub struct Slot {
    pub live: Arc<Mutex<HashMap<String, bool>>>,
    pub room_name: String,
}

impl Slot {
    pub(crate) fn assessment_finished(&self) {
        if let Some(assessing) = self
            .live
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(&self.room_name)
        {
            *assessing = false;
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.live
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.room_name);
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

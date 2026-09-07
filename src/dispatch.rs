//! Running interviewers for the rooms this process named.
//!
//! Separate from `main` because none of it is startup: the cap, the slot
//! accounting and the spawn all happen on the request path, for the length of
//! an interview, long after the binary finished deciding what it is. Here it
//! also gets tests that do not need a process.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::config::{AgentConfig, Provider};
use crate::web::RoomDispatcher;

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
    pub live: Arc<Mutex<HashSet<String>>>,
    /// From `config::max_concurrent_interviews`, so the ceiling an operator set
    /// is the ceiling this enforces.
    pub max_concurrent: usize,
}

impl RoomDispatcher for LocalDispatcher {
    fn ensure_agent(&self, room_name: &str, provider: &Provider) -> bool {
        let slot = match self.reserve(room_name) {
            Reservation::Existing => return true,
            Reservation::Full => {
                eprintln!(
                    "codetrial dispatch_refused room={room_name} reason=at_capacity limit={}",
                    self.max_concurrent
                );
                return false;
            }
            Reservation::New(slot) => slot,
        };
        let config = agent_config_for(&self.config, provider);

        // Spawned, not awaited: this runs on the request path, and the task
        // outlives the response by the length of the interview.
        self.runtime.spawn(async move {
            eprintln!("codetrial dispatch room={}", slot.room_name);
            if let Err(error) = crate::livekit::run_room(
                &config,
                &slot.room_name,
                crate::web::current_epoch_seconds(),
            )
            .await
            {
                eprintln!("codetrial agent_failed room={}: {error}", slot.room_name);
            }
        });
        true
    }
}

enum Reservation {
    Existing,
    New(Slot),
    Full,
}

impl LocalDispatcher {
    fn reserve(&self, room_name: &str) -> Reservation {
        let mut live = self.live.lock().unwrap_or_else(|error| error.into_inner());

        // A reload mints a token for the same fixed room in local mode, and two
        // agents in one room evict each other.
        if live.contains(room_name) {
            return Reservation::Existing;
        }
        if live.len() >= self.max_concurrent {
            return Reservation::Full;
        }
        live.insert(room_name.to_string());
        Reservation::New(Slot {
            live: Arc::clone(&self.live),
            room_name: room_name.to_string(),
        })
    }
}

/// One interview's claim on this process's capacity, held for as long as the
/// task that owns it.
pub struct Slot {
    pub live: Arc<Mutex<HashSet<String>>>,
    pub room_name: String,
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
    if !provider.google_api_key.is_empty() {
        config.google_api_key = provider.google_api_key.clone();
    }
    config
}

#[cfg(test)]
#[path = "../tests/unit/dispatch.rs"]
mod tests;

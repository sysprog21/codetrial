//! Running interviewers for the rooms this process named.
//!
//! Separate from main because none of it is startup: the cap, the slot
//! accounting and the spawn all happen on the request path, for the length of
//! an interview, long after the binary finished deciding what it is. Here it
//! also gets tests that do not need a process.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::config::{AgentConfig, Provider};
use crate::web::RoomDispatcher;

/// How many interviews one `codetrial web` process will host agents for at
/// once. Each is a LiveKit room plus a metered Gemini Live session, so an
/// unbounded count is an unbounded bill; a refused dispatch leaves the
/// candidate on the "Waiting" pill, which is bad, but recoverable and visible.
const MAX_CONCURRENT_INTERVIEWS: usize = 16;

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
}

impl RoomDispatcher for LocalDispatcher {
    fn ensure_agent(&self, room_name: &str, provider: &Provider) -> bool {
        {
            let mut live = self.live.lock().unwrap_or_else(|error| error.into_inner());

            // A reload mints a token for the same fixed room in local mode, and
            // two agents in one room evict each other.
            if live.contains(room_name) {
                return true;
            }
            if live.len() >= MAX_CONCURRENT_INTERVIEWS {
                eprintln!(
                    "codetrial dispatch_refused room={room_name} reason=at_capacity limit={MAX_CONCURRENT_INTERVIEWS}"
                );
                return false;
            }
            live.insert(room_name.to_string());
        }
        let config = agent_config_for(&self.config, provider);

        // Released by dropping, not by a line at the end of the task: a panic
        // in the interview would otherwise skip that line and burn the slot for
        // the life of the process, and sixteen of those refuse every interview
        // after them.
        let slot = Slot {
            live: Arc::clone(&self.live),
            room_name: room_name.to_string(),
        };

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

/// The credential half of [`select_provider`], without the pool lookup, for the
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
mod tests {
    use super::*;

    /// The capacity slot is released by dropping, so an interview that panics
    /// gives its slot back like one that ends. A cap's worth of leaked slots
    /// would refuse every interview after them for the life of the process.
    #[tokio::test]
    async fn a_panicking_interview_gives_its_capacity_back() {
        let live = Arc::new(Mutex::new(HashSet::new()));
        live.lock().unwrap().insert("interview-boom".to_string());
        let slot = Slot {
            live: Arc::clone(&live),
            room_name: "interview-boom".to_string(),
        };

        let task = tokio::spawn(async move {
            let _held = slot;
            panic!("the interview blew up");
        });
        assert!(task.await.is_err(), "the task must have panicked");

        assert!(
            live.lock().unwrap().is_empty(),
            "a panicking interview must not keep its slot"
        );
    }

    /// A provider's own Google key wins, and an empty one leaves the base key
    /// alone rather than blanking it.
    #[test]
    fn provider_credentials_replace_the_base_without_blanking_the_key() {
        let base = crate::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap();
        let mut provider = Provider {
            id: "second".to_string(),
            url: "wss://second.example".to_string(),
            api_key: "second-key".to_string(),
            api_secret: "second-secret".to_string(),
            google_api_key: String::new(),
        };

        let inherited = agent_config_for(&base, &provider);
        assert_eq!(inherited.livekit_url, "wss://second.example");
        assert_eq!(inherited.google_api_key, "primary-google");

        provider.google_api_key = "second-google".to_string();
        assert_eq!(
            agent_config_for(&base, &provider).google_api_key,
            "second-google"
        );
    }
}

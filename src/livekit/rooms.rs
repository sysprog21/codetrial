//! The agent's room administration over the LiveKit server API.
//!
//! Named apart from the rest because it talks to LiveKit over HTTP rather than
//! over the room's own event stream; the HTTP itself is `room_service`'s.
//! Everything in it exists for one problem: a previous run of this agent can
//! leave a participant behind, and a second agent in a room answers over the
//! top of the first. `run_room` and `open_session` in the parent call two of
//! these and nothing outside this module calls any of them.
//!
//! Split out of `livekit.rs` along the line its module doc already drew.

use std::time::Duration;

use crate::config::{AgentConfig, livekit_http_base};
use crate::token::livekit_room_admin_token;

use super::DUPLICATE_AGENT_ISOLATION_ATTEMPTS;
use super::room_service::{
    is_twirp_not_found, livekit_room_service_request, parse_room_service_response,
    post_room_service, room_service_failure,
};

pub(super) async fn isolate_local_agent(
    config: &AgentConfig,
    room_name: &str,
    local_identity: &str,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // A bounded number of removal rounds, then one more look. The last look is
    // what confirms the final removal landed, and writing the budget as the
    // range rather than as a counter is what keeps "one pass more" a property
    // of the shape instead of an arithmetic accident.
    for _ in 0..DUPLICATE_AGENT_ISOLATION_ATTEMPTS {
        let participants = list_room_participants(config, room_name, now_seconds).await?;
        let duplicate_agents = duplicate_agent_identities(&participants, local_identity);
        if duplicate_agents.is_empty() {
            return Ok(());
        }
        for identity in &duplicate_agents {
            eprintln!("{}", removing_duplicate_line(room_name, identity));
            remove_room_participant(config, room_name, identity, now_seconds).await?;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let participants = list_room_participants(config, room_name, now_seconds).await?;
    let duplicate_agents = duplicate_agent_identities(&participants, local_identity);
    if duplicate_agents.is_empty() {
        return Ok(());
    }

    // Loud, not fatal. This used to return Err, and in `serve` an agent error
    // aborts the web task and exits the process, so one stubborn
    // auto-dispatched agent took the whole server down mid-interview. Two
    // interviewers talking over each other is bad; no interviewer, no editor
    // and no report is worse, and the operator can see this line and act on it
    // while the candidate finishes.
    eprintln!("{}", duplicates_remained_line(room_name, &duplicate_agents));
    Ok(())
}

async fn list_room_participants(
    config: &AgentConfig,
    room_name: &str,
    now_seconds: u64,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    livekit_room_service_request(
        &config.livekit_url,
        &config.livekit_api_key,
        &config.livekit_api_secret,
        room_name,
        "ListParticipants",
        serde_json::json!({ "room": room_name }),
        now_seconds,
    )
    .await
}

/// A removal that finds nothing to remove has achieved what it was asked to do.
///
/// This used to propagate the 404 and kill the interview. The duplicate agents
/// being evicted here are the ones that leave on their own, so losing the race
/// with them is the common case, not the exceptional one: the agent greeted the
/// candidate, tried to evict a participant that had already gone, took the
/// `not_found` as fatal, and exited. The candidate was left in a room with no
/// interviewer, talking to nobody, with the status pill correctly reading
/// "Waiting" and nothing on screen explaining why.
async fn remove_room_participant(
    config: &AgentConfig,
    room_name: &str,
    identity: &str,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = livekit_room_admin_token(
        &config.livekit_api_key,
        &config.livekit_api_secret,
        room_name,
        now_seconds,
    )?;
    let (status, text) = post_room_service(
        &livekit_http_base(&config.livekit_url),
        &token,
        "RemoveParticipant",
        &serde_json::json!({ "room": room_name, "identity": identity }),
    )
    .await?;
    if is_twirp_not_found(status, &text) {
        eprintln!("{}", participant_gone_line(room_name, identity));
        return Ok(());
    }
    if !status.is_success() {
        return Err(room_service_failure("RemoveParticipant", status, &text).into());
    }
    parse_room_service_response("RemoveParticipant", &text).map(|_| ())
}

/// Proves credentials like `check-gemini` proves a Google key: the server
/// must accept a signed request. `ListRooms` needs no room to exist.
pub(crate) async fn validate_livekit_credentials(
    url: &str,
    api_key: &str,
    api_secret: &str,
    now_seconds: u64,
) -> Result<(), String> {
    let token = crate::token::livekit_room_list_token(api_key, api_secret, now_seconds)
        .map_err(|error| error.to_string())?;
    let (status, text) = post_room_service(
        &livekit_http_base(url),
        &token,
        "ListRooms",
        &serde_json::json!({}),
    )
    .await
    .map_err(|error| error.to_string())?;
    if status.is_success() {
        return Ok(());
    }

    // The body, because it is LiveKit's own account of the refusal and the
    // Setup form is the one place a reader can act on it: a status alone says
    // "did not work" to someone who already knows that. Bounded, because a
    // wrong host answers with an HTML page and the form is one line.
    const REASON_LIMIT: usize = 200;
    let reason = crate::quote::quoted_body(&text, REASON_LIMIT);
    Err(format!("LiveKit ListRooms failed: {status} {reason}"))
}

fn duplicate_agent_identities(
    participants: &serde_json::Value,
    local_identity: &str,
) -> Vec<String> {
    participants
        .get("participants")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|participant| is_agent_participant(participant))
        .filter_map(|participant| {
            participant
                .get("identity")
                .and_then(serde_json::Value::as_str)
        })
        .filter(|identity| *identity != local_identity)
        .map(ToString::to_string)
        .collect()
}

fn is_agent_participant(participant: &serde_json::Value) -> bool {
    participant
        .pointer("/permission/agent")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        || participant
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| kind == "AGENT")
}

/// The lines on the removal path below say which room they are for, since one
/// process hosts several and the removals run from tasks of their own.
fn removing_duplicate_line(room_name: &str, identity: &str) -> String {
    format!("removing duplicate LiveKit agent participant room={room_name} identity={identity}")
}

fn participant_gone_line(room_name: &str, identity: &str) -> String {
    format!("duplicate LiveKit agent participant already gone room={room_name} identity={identity}")
}

fn duplicates_remained_line(room_name: &str, identities: &[String]) -> String {
    format!(
        "WARNING: duplicate LiveKit agent participants remained after \
         {DUPLICATE_AGENT_ISOLATION_ATTEMPTS} attempts, continuing anyway: \
         room={room_name} {identities:?}"
    )
}

/// Says which room a late duplicate is being removed from.
fn eviction_line(room_name: &str, identity: &str) -> String {
    format!(
        "removing late duplicate LiveKit agent participant room={room_name} identity={identity}"
    )
}

/// Says which room a failed removal was for, since it is written from a task
/// that outlives the loop arm that knew.
fn eviction_failure_line(room_name: &str, error: &str) -> String {
    format!(
        "removing the duplicate agent failed room={room_name} ({error}); continuing the interview"
    )
}

/// A second agent in the room would answer over the top of this one, so the
/// late joiner is removed rather than tolerated.
pub(super) async fn evict_duplicate_agent(
    config: &AgentConfig,
    room_name: &str,
    identity: &str,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    eprintln!("{}", eviction_line(room_name, identity));
    remove_room_participant(config, room_name, identity, now_seconds).await
}

/// Evicts a duplicate agent from a task of its own. The room loop calls this
/// and must not wait on it: the call to LiveKit has a client timeout of thirty
/// seconds, and the loop drains the model's audio and fires the hard deadline.
/// Best effort, as it was inline: a duplicate left in place costs some
/// confusion in the room, while ending the loop over it cost the report.
///
/// Detached on purpose, unlike a side call: it makes one request that its own
/// client timeout ends within thirty seconds, writes at most one log line and
/// holds the cloned config only for that time, so an interview that ends under
/// it leaves nothing running that nothing else bounds.
pub(super) fn spawn_duplicate_eviction(
    config: &AgentConfig,
    room_name: &str,
    identity: String,
    now_seconds: u64,
) {
    let config = config.clone();
    let room_name = room_name.to_owned();
    tokio::spawn(async move {
        if let Err(error) = evict_duplicate_agent(&config, &room_name, &identity, now_seconds).await
        {
            eprintln!("{}", eviction_failure_line(&room_name, &error.to_string()));
        }
    });
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/rooms.rs"]
mod tests;

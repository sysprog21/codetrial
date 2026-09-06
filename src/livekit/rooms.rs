//! Room administration over the LiveKit server API.
//!
//! Named apart from the rest because it is the one region here that talks to
//! LiveKit over HTTP rather than over the room's own event stream. Everything
//! in it exists for one problem: a previous run of this agent can leave a
//! participant behind, and a second agent in a room answers over the top of
//! the first. `run_room` and `open_session` in the parent call two of these and
//! nothing outside this module calls any of them.
//!
//! Split out of `livekit.rs` along the line its module doc already drew.

use std::time::Duration;

use ::livekit::prelude::RemoteParticipant;

use crate::config::AgentConfig;
use crate::token::livekit_room_admin_token;

use super::DUPLICATE_AGENT_ISOLATION_ATTEMPTS;

pub(super) async fn isolate_local_agent(
    config: &AgentConfig,
    room_name: &str,
    local_identity: &str,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // One list pass more than removal rounds: the last pass only confirms the
    // final removal landed.
    let mut rounds_left = DUPLICATE_AGENT_ISOLATION_ATTEMPTS;
    loop {
        let participants = list_room_participants(config, room_name, now_seconds).await?;
        let duplicate_agents = duplicate_agent_identities(&participants, local_identity);
        if duplicate_agents.is_empty() {
            return Ok(());
        }
        if rounds_left == 0 {
            // Loud, not fatal. This used to return Err, and in `serve` an agent
            // error aborts the web task and exits the process, so one stubborn
            // auto-dispatched agent took the whole server down mid-interview.
            // Two interviewers talking over each other is bad; no interviewer,
            // no editor and no report is worse, and the operator can see this
            // line and act on it while the candidate finishes.
            eprintln!(
                "WARNING: duplicate LiveKit agent participants remained after \
                 {DUPLICATE_AGENT_ISOLATION_ATTEMPTS} attempts, continuing anyway: \
                 {duplicate_agents:?}"
            );
            return Ok(());
        }
        rounds_left -= 1;
        for identity in &duplicate_agents {
            eprintln!("removing duplicate LiveKit agent participant identity={identity}");
            remove_room_participant(config, room_name, identity, now_seconds).await?;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn list_room_participants(
    config: &AgentConfig,
    room_name: &str,
    now_seconds: u64,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    livekit_room_service_request(
        config,
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
    match livekit_room_service_request(
        config,
        room_name,
        "RemoveParticipant",
        serde_json::json!({ "room": room_name, "identity": identity }),
        now_seconds,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(error) if is_participant_gone(&error.to_string()) => {
            eprintln!("duplicate LiveKit agent participant already gone identity={identity}");
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Matches on the wire response rather than on a status code alone, because
/// LiveKit answers Twirp over HTTP and a bare 404 could also mean the route is
/// wrong. All three parts are required: the method, so a 404 from any other
/// RoomService call stays fatal; the status; and `not_found` in the body, which
/// is the participant-specific Twirp code rather than an HTML error page.
fn is_participant_gone(error: &str) -> bool {
    error.contains("RemoveParticipant") && error.contains("404") && error.contains("not_found")
}

/// The Twirp POST both RoomService callers make. Only the token and the base
/// differ, and both are strings; the body comes back with the status because a
/// refusal explains itself there and not in the code.
async fn post_room_service(
    base: &str,
    token: &str,
    method: &str,
    body: &serde_json::Value,
) -> Result<(reqwest::StatusCode, String), Box<dyn std::error::Error + Send + Sync>> {
    let response = crate::http_client()
        .post(format!("{base}/twirp/livekit.RoomService/{method}"))
        .bearer_auth(token)
        .json(body)
        .send()
        .await?;
    let status = response.status();
    Ok((status, response.text().await?))
}

async fn livekit_room_service_request(
    config: &AgentConfig,
    room_name: &str,
    method: &str,
    body: serde_json::Value,
    now_seconds: u64,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let token = livekit_room_admin_token(
        &config.livekit_api_key,
        &config.livekit_api_secret,
        room_name,
        now_seconds,
    )?;
    let (status, text) = post_room_service(
        &livekit_http_base(&config.livekit_url),
        &token,
        method,
        &body,
    )
    .await?;
    if !status.is_success() {
        return Err(format!("LiveKit RoomService {method} failed: {status} {text}").into());
    }
    parse_room_service_response(method, &text)
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
    let reason: String = text.chars().take(REASON_LIMIT).collect();
    let ellipsis = if text.chars().count() > REASON_LIMIT {
        "..."
    } else {
        ""
    };
    Err(format!(
        "LiveKit ListRooms failed: {status} {reason}{ellipsis}"
    ))
}

/// The RoomService origin for a LiveKit URL: the same host, over the scheme an
/// HTTP client will send.
///
/// The rewrite goes through `livekit_scheme` rather than matching the two
/// websocket schemes here, because `validate_livekit_url` accepts a scheme in
/// any case and this has to rewrite every spelling that accepts. A pasted
/// `WSS://` used to survive unchanged, and reqwest refuses any scheme but http
/// and https before the request leaves the process, so credentials that were
/// good came back as credentials that did not work.
///
/// The query and the fragment come off, because the Twirp path is appended
/// to what this returns and validation accepts both. A configured
/// `?region=eu` took the method name into the query rather than to the
/// service. The path itself is kept: a LiveKit behind a reverse proxy at
/// `https://host/livekit` serves RoomService under that prefix.
fn livekit_http_base(url: &str) -> String {
    let trimmed = url.trim();
    let trimmed = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    let trimmed = trimmed.trim_end_matches('/');
    match crate::config::livekit_scheme(trimmed) {
        Some((scheme, _, http)) => format!("{http}{}", &trimmed[scheme.len()..]),
        None => trimmed.to_string(),
    }
}

fn parse_room_service_response(
    method: &str,
    text: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    serde_json::from_str(text).map_err(|error| {
        format!("LiveKit RoomService {method} returned invalid JSON: {error}").into()
    })
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

/// A second agent in the room would answer over the top of this one, so the
/// late joiner is removed rather than tolerated.
pub(super) async fn evict_duplicate_agent(
    config: &AgentConfig,
    room_name: &str,
    participant: &RemoteParticipant,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let identity = participant.identity().0;
    eprintln!("removing late duplicate LiveKit agent participant identity={identity}");
    remove_room_participant(config, room_name, &identity, now_seconds).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Losing the race with a duplicate agent that left on its own must not end
    /// the interview. This shipped as a fatal error: the agent greeted the
    /// candidate, tried to evict a participant that was already gone, and
    /// exited on the `not_found`, leaving the candidate talking to an empty
    /// room. Every other RoomService failure stays fatal.
    #[test]
    fn a_removal_that_finds_nobody_is_not_a_failure() {
        assert!(is_participant_gone(
            "LiveKit RoomService RemoveParticipant failed: 404 Not Found {\"code\":\"not_found\",\"msg\":\"participant does not exist\"}"
        ));

        for still_fatal in [
            "LiveKit RoomService RemoveParticipant failed: 401 Unauthorized {\"code\":\"unauthenticated\"}",
            "LiveKit RoomService RemoveParticipant failed: 503 Service Unavailable {}",
            "LiveKit RoomService ListParticipants failed: 404 Not Found <html>no such route</html>",
        ] {
            assert!(!is_participant_gone(still_fatal), "{still_fatal}");
        }
    }

    #[test]
    fn duplicate_agent_identities_ignores_candidate_and_local_agent() {
        let participants = serde_json::json!({
            "participants": [
                {
                    "identity": "candidate-fixed",
                    "kind": "STANDARD",
                    "permission": { "agent": false }
                },
                {
                    "identity": "interviewer-interview-fixed",
                    "kind": "AGENT",
                    "permission": { "agent": true }
                },
                {
                    "identity": "hosted-agent",
                    "kind": "AGENT",
                    "permission": { "agent": true }
                },
                {
                    "identity": "legacy-agent",
                    "permission": { "agent": true }
                }
            ]
        });

        assert_eq!(
            duplicate_agent_identities(&participants, "interviewer-interview-fixed"),
            vec!["hosted-agent".to_string(), "legacy-agent".to_string()]
        );
    }

    #[test]
    fn livekit_http_base_maps_websocket_urls_to_room_service_host() {
        assert_eq!(
            livekit_http_base("wss://example.livekit.cloud/"),
            "https://example.livekit.cloud"
        );
        assert_eq!(
            livekit_http_base("ws://localhost:7880"),
            "http://localhost:7880"
        );

        // Everything `validate_livekit_url` lets through. The scheme is matched
        // case-insensitively there, and a query, a fragment and a path are all
        // accepted, so each of these is a URL a deployment can be holding.
        assert_eq!(
            livekit_http_base("WSS://example.livekit.cloud"),
            "https://example.livekit.cloud"
        );
        assert_eq!(
            livekit_http_base("Ws://localhost:7880"),
            "http://localhost:7880"
        );
        assert_eq!(
            livekit_http_base("wss://example.livekit.cloud/?region=eu"),
            "https://example.livekit.cloud"
        );

        // The path stays, because a proxy mounts RoomService under it. Only the
        // query and the fragment come off, and neither can precede a path.
        assert_eq!(
            livekit_http_base("wss://example.livekit.cloud/livekit/"),
            "https://example.livekit.cloud/livekit"
        );
        assert_eq!(
            livekit_http_base("https://example.livekit.cloud/sfu#frag"),
            "https://example.livekit.cloud/sfu"
        );

        // Shapes validation also accepts, neither of which this may mangle.
        assert_eq!(
            livekit_http_base("wss://user:pass@example.livekit.cloud/sfu"),
            "https://user:pass@example.livekit.cloud/sfu"
        );
        assert_eq!(livekit_http_base("ws://[::1]:7880"), "http://[::1]:7880");
    }

    /// `validate_livekit_url` compares the scheme without regard to case, so
    /// every spelling it accepts reaches here. The host keeps the case it was
    /// given: DNS does not care, and a path might.
    #[test]
    fn livekit_http_base_rewrites_a_scheme_in_any_case() {
        assert_eq!(
            livekit_http_base("WSS://Example.LiveKit.Cloud"),
            "https://Example.LiveKit.Cloud"
        );
        assert_eq!(
            livekit_http_base("Ws://localhost:7880/"),
            "http://localhost:7880"
        );
        assert_eq!(
            livekit_http_base("HTTPS://example.livekit.cloud"),
            "https://example.livekit.cloud"
        );
    }

    /// All three parts are required, and nothing said so.
    ///
    /// Each is load bearing for a different reason the doc comment gives: the
    /// method keeps a 404 from any other RoomService call fatal, the status
    /// distinguishes a refusal from a failure, and the Twirp code separates a
    /// participant that is gone from an HTML error page at the wrong route.
    /// Dropping any one of them widens this to a participant that is still
    /// there, and the caller treats that as a successful eviction.
    #[test]
    fn a_participant_is_gone_only_when_all_three_parts_say_so() {
        let gone = "RemoveParticipant failed: 404 {\"code\":\"not_found\"}";
        assert!(is_participant_gone(gone));

        for missing in [
            // Another RoomService call answering 404 is still fatal.
            "ListParticipants failed: 404 {\"code\":\"not_found\"}",
            // The right call and the right code, but not a refusal.
            "RemoveParticipant failed: 500 {\"code\":\"not_found\"}",
            // A 404 from the wrong route, which is an error page and not Twirp.
            "RemoveParticipant failed: 404 <html>no such route</html>",
        ] {
            assert!(!is_participant_gone(missing), "{missing}");
        }
    }

    #[test]
    fn room_service_response_rejects_malformed_success_json() {
        let error = parse_room_service_response("ListParticipants", "not json")
            .unwrap_err()
            .to_string();

        assert!(error.contains("ListParticipants returned invalid JSON"));
    }
}

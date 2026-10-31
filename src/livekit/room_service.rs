//! LiveKit's RoomService over Twirp, for any caller holding a project's URL,
//! API key and API secret.
//!
//! Split out of `rooms.rs`, where the transport read its credentials from
//! `AgentConfig`. The agent's duplicate handling still goes through it. Room
//! deletion for the web server, which has only a provider's credentials to
//! hand, is exercised by the tests and has no production caller yet.

use std::time::Duration;

use crate::config::livekit_http_base;
use crate::token::livekit_room_admin_token;

/// The Twirp POST every RoomService caller makes. Only the token and the base
/// differ, and both are strings; the body comes back with the status because a
/// refusal explains itself there and not in the code.
pub(super) async fn post_room_service(
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

/// A room-scoped call under a `roomAdmin` token, answered with LiveKit's parsed
/// reply.
pub(super) async fn livekit_room_service_request(
    url: &str,
    api_key: &str,
    api_secret: &str,
    room_name: &str,
    method: &str,
    body: serde_json::Value,
    now_seconds: u64,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let token = livekit_room_admin_token(api_key, api_secret, room_name, now_seconds)?;
    let (status, text) = post_room_service(&livekit_http_base(url), &token, method, &body).await?;
    if !status.is_success() {
        return Err(format!("LiveKit RoomService {method} failed: {status} {text}").into());
    }
    parse_room_service_response(method, &text)
}

pub(super) fn parse_room_service_response(
    method: &str,
    text: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    serde_json::from_str(text).map_err(|error| {
        format!("LiveKit RoomService {method} returned invalid JSON: {error}").into()
    })
}

/// Deletes a room, which disconnects everyone still in it.
///
/// A room that is already gone counts as deleted: cleanup runs after the fact,
/// and LiveKit drops an empty room on its own, so losing that race is the
/// ordinary case. A 2xx body still has to parse as JSON, because a reverse
/// proxy can answer 200 with an HTML page while the room is still there. Any
/// other refusal is an error carrying the method, the status and LiveKit's
/// body, the same shape as every other RoomService failure.
#[cfg(test)]
pub(crate) async fn delete_room(
    url: &str,
    api_key: &str,
    api_secret: &str,
    room_name: &str,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = crate::token::livekit_room_create_token(api_key, api_secret, now_seconds)?;
    let (status, text) = post_room_service(
        &livekit_http_base(url),
        &token,
        "DeleteRoom",
        &serde_json::json!({ "room": room_name }),
    )
    .await?;
    if is_twirp_not_found(status, &text) {
        return Ok(());
    }
    if !status.is_success() {
        return Err(format!("LiveKit RoomService DeleteRoom failed: {status} {text}").into());
    }
    parse_room_service_response("DeleteRoom", &text).map(|_| ())
}

/// How hard a production room's creation is tried before the interview starts
/// without it. The candidate is waiting on this call, so the bound is a few
/// seconds in all rather than the HTTP client's thirty.
#[derive(Clone, Copy)]
pub(super) struct RoomCreatePolicy {
    pub(super) attempts: u32,
    pub(super) attempt_timeout: Duration,
    pub(super) backoff: Duration,
}

pub(super) const ROOM_CREATE_POLICY: RoomCreatePolicy = RoomCreatePolicy {
    attempts: 2,
    attempt_timeout: Duration::from_secs(3),
    backoff: Duration::from_millis(250),
};

/// The `CreateRoom` body for an interview room.
///
/// LiveKit closes a room `departure_timeout` seconds after its last
/// non-agent participant leaves, and the default is 20. The agent is a
/// dependent participant, so an agent alone in the room does not hold it open:
/// a candidate gone for half a minute used to end the interview with no report
/// long before the absence limit. The timeout is derived from that limit plus
/// one watch tick and a margin so the product's own rule decides, not
/// LiveKit's default. `empty_timeout` is how long a room nobody ever joined
/// survives: the agent's own wait for a candidate plus a margin, so the agent's
/// no-show path fires first.
pub(super) fn interview_room_request(room_name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": room_name,
        "empty_timeout": super::interview_room_empty_timeout_secs(),
        "departure_timeout": super::interview_room_departure_timeout_secs(),
    })
}

/// Creates the interview room with its timeouts before anyone joins it.
///
/// LiveKit answers a create for a room that exists with that room, unchanged,
/// so a repeat is a success; an explicit `already_exists` or 409 from a server
/// that reports it instead counts the same. Transport failures, 429 and 5xx
/// are retried within `policy`; any other refusal (a bad credential, a bad
/// request) is returned at once, because a second try would say the same. The
/// error carries the method, status and LiveKit's body, never the token.
pub(super) async fn create_interview_room_with(
    url: &str,
    api_key: &str,
    api_secret: &str,
    room_name: &str,
    now_seconds: u64,
    policy: RoomCreatePolicy,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let token = crate::token::livekit_room_create_token(api_key, api_secret, now_seconds)?;
    let base = livekit_http_base(url);
    let body = interview_room_request(room_name);
    let mut last = String::new();
    for attempt in 0..policy.attempts {
        if attempt > 0 {
            tokio::time::sleep(policy.backoff).await;
        }
        let sent = tokio::time::timeout(
            policy.attempt_timeout,
            post_room_service(&base, &token, "CreateRoom", &body),
        )
        .await;
        let (status, text) = match sent {
            Err(_) => {
                last = "LiveKit RoomService CreateRoom timed out".to_string();
                continue;
            }
            Ok(Err(error)) => {
                last = format!("LiveKit RoomService CreateRoom failed: {error}");
                continue;
            }
            Ok(Ok(reply)) => reply,
        };
        if status.is_success() {
            return parse_room_service_response("CreateRoom", &text).map(|_| ());
        }
        if status == reqwest::StatusCode::CONFLICT || twirp_code_is(&text, "already_exists") {
            return Ok(());
        }
        last = format!("LiveKit RoomService CreateRoom failed: {status} {text}");
        if status != reqwest::StatusCode::TOO_MANY_REQUESTS && !status.is_server_error() {
            break;
        }
    }
    Err(last.into())
}

/// [`create_interview_room_with`] under the production policy.
pub(crate) async fn create_interview_room(
    url: &str,
    api_key: &str,
    api_secret: &str,
    room_name: &str,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    create_interview_room_with(
        url,
        api_key,
        api_secret,
        room_name,
        now_seconds,
        ROOM_CREATE_POLICY,
    )
    .await
}

/// Both the status and the Twirp code, because a bare 404 could also be a
/// wrong host or path answering with an HTML page, and that is a failure to
/// report rather than a room or participant to forget.
pub(super) fn is_twirp_not_found(status: reqwest::StatusCode, text: &str) -> bool {
    status == reqwest::StatusCode::NOT_FOUND && twirp_code_is(text, "not_found")
}

/// Whether a Twirp error body carries `code`.
fn twirp_code_is(text: &str, code: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text).is_ok_and(|body| body["code"] == code)
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/room_service.rs"]
mod tests;

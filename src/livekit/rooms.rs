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

use crate::config::AgentConfig;
use crate::token::livekit_room_admin_token;

use super::DUPLICATE_AGENT_ISOLATION_ATTEMPTS;

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
            eprintln!("removing duplicate LiveKit agent participant identity={identity}");
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
    eprintln!(
        "WARNING: duplicate LiveKit agent participants remained after \
         {DUPLICATE_AGENT_ISOLATION_ATTEMPTS} attempts, continuing anyway: \
         {duplicate_agents:?}"
    );
    Ok(())
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
    identity: &str,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    eprintln!("removing late duplicate LiveKit agent participant identity={identity}");
    remove_room_participant(config, room_name, identity, now_seconds).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A RoomService that answers what the test tells it to and remembers what
    /// it was asked.
    ///
    /// A real socket rather than an injected transport, because what these
    /// functions do with a reply is bound up with how they make the request:
    /// the Twirp path, the bearer token and the status handling are the parts
    /// worth pinning, and none of them survives being stubbed out one layer up.
    /// Replies are answered in order and the last one repeats, so a caller that
    /// polls needs one entry rather than twenty.
    struct RoomService {
        url: String,
        seen: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl RoomService {
        async fn start(replies: Vec<(u16, &'static str)>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let seen = Arc::new(Mutex::new(Vec::new()));
            let recorded = seen.clone();
            tokio::spawn(async move {
                let mut replies = replies.into_iter().peekable();
                let mut last = (200, "{}");
                loop {
                    let Ok((mut socket, _)) = listener.accept().await else {
                        return;
                    };
                    if replies.peek().is_some() {
                        last = replies.next().unwrap();
                    }

                    // Headers to the blank line, then exactly the declared
                    // body.
                    let mut raw = Vec::new();
                    let mut byte = [0u8; 1];
                    while !raw.ends_with(b"\r\n\r\n") {
                        match socket.read(&mut byte).await {
                            Ok(0) | Err(_) => break,
                            Ok(_) => raw.push(byte[0]),
                        }
                    }
                    let head = String::from_utf8_lossy(&raw).to_string();
                    let path = head
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or_default()
                        .to_string();
                    let length: usize = head
                        .to_ascii_lowercase()
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(0);
                    let mut body = vec![0u8; length];
                    if length > 0 && socket.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    recorded
                        .lock()
                        .unwrap()
                        .push((path, String::from_utf8_lossy(&body).to_string()));

                    let (status, reply) = last;
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                }
            });
            Self { url, seen }
        }

        fn config(&self) -> AgentConfig {
            crate::config::load_from_pairs([
                ("LIVEKIT_URL", self.url.as_str()),
                ("LIVEKIT_API_KEY", "devkey"),
                ("LIVEKIT_API_SECRET", "devsecret"),
                ("GOOGLE_API_KEY", "google"),
            ])
            .unwrap()
        }

        fn requests(&self) -> Vec<(String, String)> {
            self.seen.lock().unwrap().clone()
        }
    }

    /// The request this makes, and what it does with each kind of answer.
    ///
    /// The Twirp path is a contract with LiveKit that no test named: a wrong
    /// method segment reaches a real server and 404s, which
    /// `is_participant_gone` then has to be careful not to read as a departed
    /// participant. The success path returned the parsed body and the failure
    /// path had to keep the method, the status and the body, because that
    /// string is the only thing the caller can tell them apart by.
    #[tokio::test]
    async fn a_room_service_call_names_its_method_and_carries_the_answer() {
        let service =
            RoomService::start(vec![(200, r#"{"participants":[{"identity":"a"}]}"#)]).await;
        let config = service.config();

        let participants = list_room_participants(&config, "interview-abc12345", 1_700_000_000)
            .await
            .unwrap();
        assert_eq!(participants["participants"][0]["identity"], "a");

        let (path, body) = service.requests().into_iter().next().unwrap();
        assert_eq!(path, "/twirp/livekit.RoomService/ListParticipants");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["room"],
            "interview-abc12345"
        );

        // A refusal is an error, and the message carries all three parts the
        // caller reads it for.
        let refusing = RoomService::start(vec![(503, r#"{"msg":"busy"}"#)]).await;
        let error = list_room_participants(&refusing.config(), "interview-abc12345", 1_700_000_000)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("ListParticipants"), "{error}");
        assert!(error.contains("503"), "{error}");
        assert!(error.contains("busy"), "{error}");
    }

    /// The Setup form's credential check, and the sentence it hands back.
    ///
    /// This is what a reader acts on: the form shows LiveKit's own account of
    /// the refusal, because a status alone says "did not work" to somebody who
    /// already knows that. It is bounded because a wrong host answers with an
    /// HTML page and the form is one line, and the boundary is the part worth
    /// pinning: an off-by-one there either truncates a reason that fit or lets
    /// a whole page through.
    #[tokio::test]
    async fn a_credential_check_reports_what_livekit_said() {
        let accepted = RoomService::start(vec![(200, "{}")]).await;
        validate_livekit_credentials(&accepted.url, "devkey", "devsecret", 1_700_000_000)
            .await
            .expect("a project that answers ListRooms has usable credentials");
        let (path, _) = accepted.requests().into_iter().next().unwrap();
        assert_eq!(path, "/twirp/livekit.RoomService/ListRooms");

        let refused = RoomService::start(vec![(401, r#"{"msg":"invalid api key"}"#)]).await;
        let error = validate_livekit_credentials(&refused.url, "devkey", "wrong", 1_700_000_000)
            .await
            .unwrap_err();
        assert!(error.contains("401"), "{error}");
        assert!(error.contains("invalid api key"), "{error}");
        assert!(
            !error.ends_with("..."),
            "a reason that fits is not truncated"
        );
    }

    /// A wrong host answers with a page, and the form takes one line of it.
    #[tokio::test]
    async fn a_credential_check_bounds_the_reason_it_repeats() {
        // Longer than the limit by one, which is the only length that tells the
        // boundary apart from its neighbours.
        const OVER: &str = concat!(
            "0123456789012345678901234567890123456789012345678901234567890123456789",
            "0123456789012345678901234567890123456789012345678901234567890123456789",
            "0123456789012345678901234567890123456789012345678901234567890123456789",
        );
        assert_eq!(OVER.len(), 210);

        let wordy = RoomService::start(vec![(500, OVER)]).await;
        let error = validate_livekit_credentials(&wordy.url, "devkey", "devsecret", 1_700_000_000)
            .await
            .unwrap_err();
        assert!(
            error.ends_with("..."),
            "an over-long reason is cut and marked"
        );
        assert!(
            error.contains(&OVER[..200]) && !error.contains(&OVER[..201]),
            "exactly the limit is repeated: {error}"
        );

        // Exactly the limit, which is the length that tells `>` from `>=`: a
        // reason that fits to the last character is whole, not cut.
        let exact = &OVER[..200];
        let fitting = RoomService::start(vec![(500, exact)]).await;
        let error =
            validate_livekit_credentials(&fitting.url, "devkey", "devsecret", 1_700_000_000)
                .await
                .unwrap_err();
        assert!(
            error.ends_with(exact),
            "a reason of exactly the limit is not marked as cut: {error}"
        );
    }

    /// A late duplicate is evicted by name.
    ///
    /// This runs when a second agent joins after isolation has already passed,
    /// which is the case isolation cannot cover: the room was clear when it
    /// looked. It takes an identity rather than the participant it came from,
    /// because the identity is all it needs and a participant cannot be built
    /// to test with.
    #[tokio::test]
    async fn a_late_duplicate_is_evicted_by_name() {
        let service = RoomService::start(vec![(200, "{}")]).await;

        evict_duplicate_agent(
            &service.config(),
            "interview-abc12345",
            "agent-late",
            1_700_000_000,
        )
        .await
        .unwrap();

        let (path, body) = service.requests().into_iter().next().unwrap();
        assert_eq!(path, "/twirp/livekit.RoomService/RemoveParticipant");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["identity"],
            "agent-late"
        );
    }

    /// A removal that finds nobody is done, and every other refusal is not.
    ///
    /// The predicate is unit tested above; this is the path that consults it,
    /// which is where losing the race with a departing agent used to end the
    /// interview.
    #[tokio::test]
    async fn a_removal_consults_the_predicate_before_giving_up() {
        let gone = RoomService::start(vec![(
            404,
            r#"{"code":"not_found","msg":"participant does not exist"}"#,
        )])
        .await;
        remove_room_participant(
            &gone.config(),
            "interview-abc12345",
            "agent-old",
            1_700_000_000,
        )
        .await
        .expect("a participant that already left is not a failure");
        let (path, _) = gone.requests().into_iter().next().unwrap();
        assert_eq!(path, "/twirp/livekit.RoomService/RemoveParticipant");

        let refused = RoomService::start(vec![(401, r#"{"code":"unauthenticated"}"#)]).await;
        remove_room_participant(
            &refused.config(),
            "interview-abc12345",
            "agent-old",
            1_700_000_000,
        )
        .await
        .expect_err("a refusal that is not a departed participant stays fatal");
    }

    /// Isolation lists, removes what it finds, and stops.
    ///
    /// The loop is the part worth pinning: it re-lists after removing, so the
    /// last pass is the one that confirms the room is clear, and it gives up
    /// loudly rather than spinning forever. Nothing asserted that it removed
    /// anything at all.
    #[tokio::test]
    async fn isolation_removes_a_duplicate_and_confirms_the_room_is_clear() {
        // Crowded once, then empty, which is the ordinary case: list, remove,
        // list again and find nothing.
        let service = RoomService::start(vec![
            (
                200,
                r#"{"participants":[{"identity":"agent-old","kind":"AGENT"},{"identity":"agent-me","kind":"AGENT"}]}"#,
            ),
            (200, r#"{}"#),
            (200, r#"{"participants":[]}"#),
        ])
        .await;

        isolate_local_agent(
            &service.config(),
            "interview-abc12345",
            "agent-me",
            1_700_000_000,
        )
        .await
        .unwrap();

        let methods: Vec<String> = service
            .requests()
            .into_iter()
            .map(|(path, _)| path.rsplit('/').next().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            methods,
            vec![
                "ListParticipants".to_string(),
                "RemoveParticipant".to_string(),
                "ListParticipants".to_string(),
            ],
            "it lists, removes what it found, and lists again to confirm"
        );
    }

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

//! The `tests` module of `src/livekit/room_service.rs`, which declares this
//! file by path, so private items are in scope through `super`.

use super::*;
use axum::{
    Router,
    http::{HeaderMap, StatusCode, Uri, header},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::sync::{Arc, Mutex};

const STUB_API_KEY: &str = "devkey";
const STUB_API_SECRET: &str = "devsecret";
const NOW: u64 = 1_700_000_000;

/// One request as the stub saw it.
struct Seen {
    path: String,
    bearer: Option<String>,
    body: serde_json::Value,
}

/// A RoomService that gives every request the same answer and remembers what
/// it was asked. The URL is `ws://` because that is what an operator
/// configures, so `delete_room` has to map it to HTTP before the request can
/// arrive here.
struct RoomService {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl RoomService {
    async fn start(status: u16, reply: &'static str) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: String| {
            let recorded = Arc::clone(&recorded);
            async move {
                recorded.lock().unwrap().push(Seen {
                    path: uri.path().to_string(),
                    bearer: headers
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.strip_prefix("Bearer "))
                        .map(str::to_string),
                    body: serde_json::from_str(&body).unwrap_or_default(),
                });
                (
                    StatusCode::from_u16(status).unwrap(),
                    [(header::CONTENT_TYPE, "application/json")],
                    reply,
                )
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, seen }
    }

    async fn delete(&self, room: &str) -> Result<(), String> {
        delete_room(&self.url, STUB_API_KEY, STUB_API_SECRET, room, NOW)
            .await
            .map_err(|error| error.to_string())
    }

    fn requests(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.seen.lock().unwrap())
    }
}

/// The `video` grant of a token this project's secret signed, or `None` when
/// the issuer or the signature is wrong.
fn verified_video_grant(token: &str) -> Option<serde_json::Value> {
    if crate::token::livekit_token_issuer(token).as_deref() != Some(STUB_API_KEY) {
        return None;
    }
    let (signing_input, signature) = token.rsplit_once('.')?;
    if !crate::token::verify_hs256(STUB_API_SECRET, signing_input, signature) {
        return None;
    }
    let (_, payload) = signing_input.split_once('.')?;
    let claims: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    claims.get("video").cloned()
}

/// LiveKit gates DeleteRoom on `roomCreate`, so that is the grant the call has
/// to carry. `roomAdmin` is pinned absent because it is the grant a caller
/// would reach for by habit, and it would let the cleanup path evict people.
#[tokio::test]
async fn deleting_a_room_posts_delete_room_under_a_room_create_token() {
    let service = RoomService::start(200, "{}").await;

    service.delete("interview-abc12345").await.unwrap();

    let requests = service.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.path, "/twirp/livekit.RoomService/DeleteRoom");
    assert_eq!(
        request.body,
        serde_json::json!({ "room": "interview-abc12345" })
    );

    let token = request.bearer.as_deref().expect("a bearer token");
    let video = verified_video_grant(token).expect("signed by this project");
    assert_eq!(video["roomCreate"], serde_json::json!(true));
    assert_eq!(video["roomAdmin"], serde_json::Value::Null);
}

/// Cleanup runs after the fact and LiveKit drops empty rooms on its own, so a
/// room that is already gone is the ordinary outcome, not a failure.
#[tokio::test]
async fn a_room_already_gone_counts_as_deleted() {
    let service = RoomService::start(
        404,
        r#"{"code":"not_found","msg":"requested room does not exist"}"#,
    )
    .await;

    assert_eq!(service.delete("interview-abc12345").await, Ok(()));
}

/// Anything else is an error that says which call failed, how, and what
/// LiveKit said. A 404 without the Twirp code is in this group: it is what a
/// wrong host or path answers, and forgetting the room then would leak it.
#[tokio::test]
async fn any_other_refusal_is_an_error_that_names_the_call() {
    let failing = RoomService::start(500, r#"{"code":"internal","msg":"boom"}"#).await;
    let error = failing.delete("interview-abc12345").await.unwrap_err();
    assert!(error.contains("DeleteRoom"), "{error}");
    assert!(error.contains("500"), "{error}");
    assert!(error.contains("boom"), "{error}");

    let wrong_route = RoomService::start(404, "404 page not found").await;
    let error = wrong_route.delete("interview-abc12345").await.unwrap_err();
    assert!(error.contains("404"), "{error}");
}

/// A proxy can answer 200 with an HTML page while the room is still there, so
/// a success that is not JSON is not a deleted room.
#[tokio::test]
async fn a_html_success_is_not_a_deleted_room() {
    let service = RoomService::start(200, "<html>ok</html>").await;
    let error = service.delete("interview-abc12345").await.unwrap_err();
    assert!(error.contains("DeleteRoom"), "{error}");
    assert!(error.contains("invalid JSON"), "{error}");
}

#[test]
fn a_room_is_gone_only_on_a_twirp_not_found() {
    let not_found = reqwest::StatusCode::NOT_FOUND;
    assert!(is_twirp_not_found(
        not_found,
        r#"{"code":"not_found","msg":"x"}"#
    ));
    assert!(!is_twirp_not_found(not_found, "not_found"));
    assert!(!is_twirp_not_found(not_found, r#"{"code":"internal"}"#));
    assert!(!is_twirp_not_found(
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        r#"{"code":"not_found"}"#
    ));
}

// Creating the interview room with its own timeouts.

const FAST_POLICY: RoomCreatePolicy = RoomCreatePolicy {
    attempts: 2,
    attempt_timeout: Duration::from_millis(500),
    backoff: Duration::from_millis(5),
};

/// A RoomService that answers each request from a script, repeating the last
/// entry once it runs out, and remembers what it was asked.
struct ScriptedRoomService {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl ScriptedRoomService {
    async fn start(script: Vec<(u16, &'static str)>) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = std::sync::Arc::clone(&seen);
        let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: String| {
            let recorded = std::sync::Arc::clone(&recorded);
            let script = script.clone();
            async move {
                let mut seen = recorded.lock().unwrap();
                let index = seen.len().min(script.len() - 1);
                seen.push(Seen {
                    path: uri.path().to_string(),
                    bearer: headers
                        .get(header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.strip_prefix("Bearer "))
                        .map(str::to_string),
                    body: serde_json::from_str(&body).unwrap_or_default(),
                });
                let (status, reply) = script[index];
                (
                    StatusCode::from_u16(status).unwrap(),
                    [(header::CONTENT_TYPE, "application/json")],
                    reply,
                )
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self { url, seen }
    }

    async fn create(&self, room: &str) -> Result<(), String> {
        create_interview_room_with(
            &self.url,
            STUB_API_KEY,
            STUB_API_SECRET,
            room,
            NOW,
            FAST_POLICY,
        )
        .await
        .map_err(|error| error.to_string())
    }

    fn requests(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.seen.lock().unwrap())
    }
}

/// The exact body a production room is created with. The departure timeout is
/// the absence limit (90 s) plus one watch tick (2 s) plus an 8 s margin
/// (100 s), and the empty timeout is the agent's candidate wait plus a 30 s
/// margin (330 s). Without `departure_timeout` LiveKit closes the room 20 s
/// after the candidate leaves.
#[tokio::test]
async fn creating_an_interview_room_posts_the_exact_timeouts_under_a_room_create_token() {
    let service = ScriptedRoomService::start(vec![(200, "{}")]).await;

    service.create("interview-abc12345").await.unwrap();

    let requests = service.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/twirp/livekit.RoomService/CreateRoom");
    assert_eq!(
        requests[0].body,
        serde_json::json!({
            "name": "interview-abc12345",
            "empty_timeout": 330,
            "departure_timeout": 100,
        })
    );
    let token = requests[0].bearer.as_deref().expect("a bearer token");
    let video = verified_video_grant(token).expect("signed by this project");
    assert_eq!(video["roomCreate"], serde_json::json!(true));
    assert_eq!(video["roomAdmin"], serde_json::Value::Null);
}

/// The relationship the 20 s default broke: the room outlives the absence
/// limit by at least one watch tick, and both numbers come from the constants.
#[test]
fn the_room_departure_timeout_covers_the_absence_limit_and_one_watch_tick() {
    let departure = u64::from(crate::livekit::interview_room_departure_timeout_secs());
    let limit = crate::livekit::CANDIDATE_ABSENCE_LIMIT.as_secs();
    assert!(
        departure
            >= limit
                + u64::from(crate::livekit::ceil_secs(Duration::from_secs_f64(
                    crate::agent::WATCH_TICK_S
                )))
    );
    assert!(departure > 20, "LiveKit's own default is 20 s");
    assert!(
        u64::from(crate::livekit::interview_room_empty_timeout_secs())
            > crate::livekit::CANDIDATE_JOIN_LIMIT.as_secs()
    );
    assert_eq!(
        interview_room_request("r")["departure_timeout"],
        serde_json::json!(departure)
    );
}

/// An existing room is returned by LiveKit as a success; a server that reports
/// it as a conflict is the same outcome, and neither is retried.
#[tokio::test]
async fn an_existing_room_counts_as_created() {
    for (status, reply) in [
        (200, r#"{"name":"interview-abc12345"}"#),
        (409, "conflict"),
        (400, r#"{"code":"already_exists","msg":"room exists"}"#),
    ] {
        let service = ScriptedRoomService::start(vec![(status, reply)]).await;
        assert_eq!(
            service.create("interview-abc12345").await,
            Ok(()),
            "{status}"
        );
        assert_eq!(service.requests().len(), 1, "{status}");
    }
}

/// A transient failure is retried once and the second answer decides.
#[tokio::test]
async fn a_transient_refusal_is_retried_once() {
    for first in [500, 503, 429] {
        let service =
            ScriptedRoomService::start(vec![(first, r#"{"code":"unavailable"}"#), (200, "{}")])
                .await;
        assert_eq!(
            service.create("interview-abc12345").await,
            Ok(()),
            "{first}"
        );
        assert_eq!(service.requests().len(), 2, "{first}");
    }

    let failing =
        ScriptedRoomService::start(vec![(500, r#"{"code":"internal","msg":"boom"}"#)]).await;
    let error = failing.create("interview-abc12345").await.unwrap_err();
    assert!(error.contains("CreateRoom"), "{error}");
    assert!(error.contains("500"), "{error}");
    assert!(error.contains("boom"), "{error}");
    assert_eq!(failing.requests().len(), 2);
}

/// A refused credential or request says the same thing twice, so it is
/// returned after one try, and the error never carries the bearer token.
#[tokio::test]
async fn a_permanent_refusal_is_not_retried_and_does_not_leak_the_token() {
    let service =
        ScriptedRoomService::start(vec![(401, r#"{"code":"unauthenticated","msg":"no"}"#)]).await;
    let error = service.create("interview-abc12345").await.unwrap_err();
    assert!(error.contains("401"), "{error}");
    let requests = service.requests();
    assert_eq!(requests.len(), 1);
    let token = requests[0].bearer.as_deref().unwrap();
    assert!(!error.contains(token), "{error}");
    assert!(!error.contains(STUB_API_SECRET), "{error}");
}

/// A server that cannot be reached is a failure to report, not a hang.
#[tokio::test]
async fn an_unreachable_room_service_fails_within_the_policy() {
    let started = std::time::Instant::now();
    let error = create_interview_room_with(
        "ws://127.0.0.1:1",
        STUB_API_KEY,
        STUB_API_SECRET,
        "interview-abc12345",
        NOW,
        FAST_POLICY,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("CreateRoom"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

/// A server that accepts and never answers is cut off by the per-attempt
/// timeout on every attempt, so the call returns within the policy's bound
/// instead of waiting on the HTTP client's thirty seconds.
#[tokio::test]
async fn a_room_service_that_never_answers_times_out_on_every_attempt() {
    let app = Router::new().fallback(|| async {
        tokio::time::sleep(Duration::from_secs(30)).await;
        "{}"
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let policy = RoomCreatePolicy {
        attempts: 2,
        attempt_timeout: Duration::from_millis(100),
        backoff: Duration::from_millis(5),
    };
    let started = std::time::Instant::now();
    let error = create_interview_room_with(
        &url,
        STUB_API_KEY,
        STUB_API_SECRET,
        "interview-abc12345",
        NOW,
        policy,
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("timed out"), "{error}");
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(200), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}

/// The cost contract quotes the start's added wait from these three numbers,
/// and the tests above all run faster policies, so nothing else would notice
/// one of them drifting.
#[test]
fn the_production_policy_bounds_the_start_to_six_and_a_quarter_seconds() {
    let policy = ROOM_CREATE_POLICY;
    assert_eq!(policy.attempts, 2);
    assert_eq!(policy.attempt_timeout, Duration::from_secs(3));
    assert_eq!(policy.backoff, Duration::from_millis(250));
    let worst = policy.attempt_timeout * policy.attempts + policy.backoff * (policy.attempts - 1);
    assert_eq!(worst, Duration::from_millis(6250));
}

/// Every duration the timeouts are built from is whole seconds, so only a
/// fraction shows that the conversion rounds up and not down: a timeout
/// rounded down could undercut the grace it is derived from.
#[test]
fn a_fractional_timeout_rounds_up_to_whole_seconds() {
    let ceil = crate::livekit::ceil_secs;
    assert_eq!(ceil(Duration::from_millis(1_500)), 2);
    assert_eq!(ceil(Duration::from_nanos(1)), 1);
    assert_eq!(ceil(Duration::from_secs(3)), 3);
    assert_eq!(ceil(Duration::ZERO), 0);
    assert_eq!(ceil(Duration::from_secs(u64::MAX)), u32::MAX);
}

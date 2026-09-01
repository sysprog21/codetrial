//! Minting the LiveKit tokens a page joins a room with, for candidates and
//! observers alike, plus the per-client rate limit that decides who gets to
//! ask. Distinct from `crate::token`, which does the signing.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use ring::rand::SecureRandom;
use serde_json::{Value, json};

use crate::accounts::{blocking, release_interview_room};
use crate::config::{DEFAULT_DURATION_MIN, MAX_DURATION_MIN, MIN_DURATION_MIN};
use crate::current_epoch_seconds;
use crate::token::{LivekitTokenInput, TOKEN_TTL_SECONDS, livekit_observer_token, livekit_token};

use super::auth::current_user;
use super::consent::{checked_consent, claim_consent};
use super::pool::{ProviderChoice, room_and_available_provider};
use super::{
    AppState, MAX_BODY_BYTES, client_ip, json_response, number_json, rate_limited_response,
    unauthorized_response,
};

/// No `Debug`, for the reason [`crate::config::AgentConfig`] has none: one
/// field is a credential, and a type that can print it will eventually be
/// printed. Nothing needs to format this, so the derive is the whole risk.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenConfig<'a> {
    pub api_key: &'a str,
    pub api_secret: &'a str,
    pub server_url: &'a str,
    /// The deployment's recording cap, and `None` wherever recording is off.
    /// An interview may not outlast the recording made of it: `stale_after` in
    /// recording/sweeper.rs reaps one still running past this many minutes, so
    /// a longer interview loses exactly the stretch it was recorded for.
    pub recording_max_min: Option<u32>,
}

/// Same reason, and the sharpest case of it: `token` is a minted LiveKit
/// credential a holder can join a room with, not a key that merely mints them.
/// It is the shortest-lived secret here and the easiest to print by accident,
/// because it looks like a response body rather than a key.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenResponse {
    pub token: String,
    pub server_url: String,
    pub room_name: String,
}

/// `/api/token` mints a real LiveKit credential and is necessarily
/// unauthenticated: the browser has nowhere to keep a secret. Cap it per client
/// so a stray script cannot drain the account's room minutes.
///
/// The ceiling, so nobody has to rediscover it from a bill: the counter lives
/// in this process, so the number an attacker actually gets is this times the
/// number of web processes behind the load balancer. `codetrial web` is built
/// to run as the web half of a split deployment, so more than one is a
/// supported shape rather than a hypothetical.
///
/// Left per process on purpose. A shared counter means every token request
/// takes a write lock on the account database, which is the one thing the
/// single guarded connection in `crate::accounts` is not built to absorb, and
/// the failure this bounds is a stray script rather than a determined
/// attacker: the factor is N, not unbounded. Move it into SQLite when N stops
/// being small enough to multiply in your head.
pub const TOKEN_RATE_LIMIT: u32 = 30;

pub(crate) const TOKEN_RATE_WINDOW: Duration = Duration::from_secs(60);

/// The key is what the route can name a caller by. `/api/token` and
/// `/api/login` are reachable without a credential, so they key on the address;
/// replay ingestion runs behind `Owner` and keys on the account. The window is
/// shared because all three buckets want the same minute.
#[derive(Clone)]
pub(crate) struct TokenRateLimit<K = IpAddr> {
    state: Arc<Mutex<RateLimitState<K>>>,
    limit: u32,
}

struct RateLimitState<K> {
    windows: HashMap<K, Window>,
    last_sweep: Instant,
}

impl<K> Default for RateLimitState<K> {
    fn default() -> Self {
        Self {
            windows: HashMap::new(),
            last_sweep: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Window {
    started: Instant,
    count: u32,
}

impl<K: Eq + std::hash::Hash> TokenRateLimit<K> {
    pub(crate) fn with_limit(limit: u32) -> Self {
        Self {
            state: Arc::default(),
            limit,
        }
    }

    /// Crosses a module boundary: the token handlers, the login handler in
    /// `super::auth` and the replay handler in `super::interviews` each hold
    /// their own limiter of this type.
    pub(crate) fn allow(&self, client: K, now: Instant) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());

        // Sweeping every tracked client on every call cost O(clients) per
        // request while holding the lock, so a flood from many addresses was
        // absorbed by the limiter into more work per request: the control
        // amplifying what it exists to damp. The entry being touched expires
        // itself below, and the sweep that keeps the map bounded now runs at
        // most once per window.
        if now.duration_since(state.last_sweep) >= TOKEN_RATE_WINDOW {
            state
                .windows
                .retain(|_, window| now.duration_since(window.started) < TOKEN_RATE_WINDOW);
            state.last_sweep = now;
        }
        let window = state.windows.entry(client).or_insert(Window {
            started: now,
            count: 0,
        });
        if now.duration_since(window.started) >= TOKEN_RATE_WINDOW {
            *window = Window {
                started: now,
                count: 0,
            };
        }
        window.count += 1;
        window.count <= self.limit
    }
}

#[derive(Clone, Copy)]
struct RoomAuthorization {
    owner_id: i64,
    expires_at: u64,
}

/// Who may watch which room, and for how long.
///
/// Granting sweeps and reading enforces expiry per entry, for the reason
/// [`TokenRateLimit::allow`] gives: a sweep on the read is O(rooms) under the
/// lock on the request path. It also put the pruning on the rare operation, so
/// a server nobody observed never swept and the map grew for every token it
/// minted.
#[derive(Default)]
pub(crate) struct RoomAuthorizations {
    grants: HashMap<String, RoomAuthorization>,
    last_sweep: u64,
}

impl RoomAuthorizations {
    /// Records that `owner_id` may observe `room_name` until their token
    /// expires.
    pub(crate) fn grant(&mut self, room_name: String, owner_id: i64, now: u64) {
        // At most once per token lifetime, because nothing in the map can
        // expire faster than that and a sweep that finds nothing is pure cost.
        if now.saturating_sub(self.last_sweep) >= TOKEN_TTL_SECONDS {
            self.grants.retain(|_, grant| grant.expires_at > now);
            self.last_sweep = now;
        }
        self.grants.insert(
            room_name,
            RoomAuthorization {
                owner_id,
                expires_at: now + TOKEN_TTL_SECONDS,
            },
        );
    }

    /// Whether `owner_id` may observe `room_name` right now. A grant left
    /// behind by a sweep that has not run yet is still expired.
    pub(crate) fn allows(&self, room_name: &str, owner_id: i64, now: u64) -> bool {
        self.grants
            .get(room_name)
            .is_some_and(|grant| grant.owner_id == owner_id && grant.expires_at > now)
    }
}

pub fn token_response(
    config: &TokenConfig<'_>,
    body: &[u8],
    room_name: &str,
    default_identity: &str,
    now_seconds: u64,
) -> Result<TokenResponse, Box<dyn std::error::Error + Send + Sync>> {
    // No body means "give me the defaults". A body that is present but broken
    // is a client bug, and silently handing back a Two Sum interview hides it.
    let request = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice::<Value>(body)?
    };
    let problem_id = request
        .get("problemId")
        .and_then(Value::as_str)
        .unwrap_or(crate::agent::DEFAULT_PROBLEM_ID);
    let duration_min = token_duration_min(request.get("durationMin"), config.recording_max_min);
    let interview_loop =
        crate::agent::InterviewLoop::parse(request.get("interviewLoop").and_then(Value::as_str));
    let profile = crate::agent::sanitize_interview_profile(request.get("interviewProfile"));
    let grounding = crate::agent::sanitize_interview_grounding(request.get("interviewGrounding"));
    let mut metadata = json!({
        "problemId": problem_id,
        "durationMin": duration_min,
        "interviewLoop": interview_loop.as_str(),
        "interviewProfile": crate::agent::interview_profile_json(&profile),
        "candidateIdentity": default_identity,
    });
    if !grounding.is_empty() {
        metadata
            .as_object_mut()
            .expect("metadata is an object")
            .insert(
                "interviewGrounding".to_string(),
                crate::agent::interview_grounding_json(&grounding),
            );
    }
    let metadata = metadata.to_string();

    Ok(TokenResponse {
        token: livekit_token(LivekitTokenInput {
            api_key: config.api_key,
            api_secret: config.api_secret,
            name: "Candidate",
            identity: default_identity,
            room: room_name,
            metadata: &metadata,
            now_seconds,
            agent: false,
        })?,
        server_url: config.server_url.to_string(),
        room_name: room_name.to_string(),
    })
}

/// The server, rather than the browser, mints the identity the room agent uses
/// to recognize its candidate.
pub(crate) fn generated_candidate_identity() -> String {
    format!("candidate-{}", suffix(CANDIDATE_SUFFIX_LEN))
}

pub(crate) const CANDIDATE_SUFFIX_LEN: usize = 6;

/// The longest interview this deployment will start, which is its recording's
/// bound wherever it records: the sweeper reaps a recording that outlives
/// `max_minutes`, so a longer interview loses the stretch past it.
///
/// One function because two callers need the same answer and must not drift.
/// `/api/token` enforces it, and `/api/session` hands it to the lobby so the
/// browser stops offering what this server would silently shorten.
///
/// Clamped into the range rather than taken as given: a cap above
/// `MAX_DURATION_MIN` does not buy a longer interview than the endpoint has
/// ever offered, and one below `MIN_DURATION_MIN` would cross the floor, which
/// is a panic in `f64::clamp` rather than a short interview.
pub(crate) fn duration_ceiling(recording_max_min: Option<u32>) -> u32 {
    recording_max_min.map_or(MAX_DURATION_MIN, |cap| {
        cap.clamp(MIN_DURATION_MIN, MAX_DURATION_MIN)
    })
}

pub(crate) fn token_duration_min(value: Option<&Value>, recording_max_min: Option<u32>) -> Value {
    let ceiling = duration_ceiling(recording_max_min);
    let duration = value.and_then(crate::agent::json_number).unwrap_or(0.0);
    if duration == 0.0 || duration.is_nan() {
        // The default is a request like any other where the cap is shorter than
        // it. Handing back a length this server cannot record, for a caller who
        // named no length at all, is the same lost stretch of interview.
        json!(DEFAULT_DURATION_MIN.min(ceiling))
    } else {
        // Fractional minutes stay fractional: the metadata is echoed back to
        // the frontend, which renders whatever the lobby asked for.
        number_json(duration.clamp(MIN_DURATION_MIN.into(), ceiling.into()))
    }
}

pub(crate) async fn token_handler(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response {
    // Ahead of the rate limit on purpose: the bucket is keyed by address, so
    // letting anonymous callers spend it would let them lock out the signed-in
    // candidate sharing their NAT. A refused request mints nothing.
    //
    // Where accounts exist, an interview belongs to one. Hiding the start
    // button would not stop anyone opening /interview directly, so the gate
    // lives on the credential.
    let Some(accounts) = state.accounts.clone() else {
        return state.accounts_error();
    };
    let user = match current_user(state.accounts.as_ref(), request.headers()).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            return json_response(
                StatusCode::UNAUTHORIZED,
                json!({ "error": "Enter your GitHub username to start an interview." }),
            );
        }
        Err(_) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read account session." }),
            );
        }
    };

    // Where recording is on, the interview produces a file that has to be
    // delivered to a person, and a self-declared handle is not one. Refusing
    // here rather than at delivery time is the difference between a candidate
    // who cannot start and a candidate whose finished interview cannot be sent
    // anywhere.
    if state.config.recording.is_some() && user.verified_email.is_none() {
        return json_response(
            StatusCode::FORBIDDEN,
            json!({
                "code": "recording_requires_verified_identity",
                "error": "This interview is recorded, so it needs a GitHub sign-in. A typed username cannot receive the recording."
            }),
        );
    }

    let client = client_ip(request.headers(), peer, state.config.trusted_proxy_hops);
    if !state.token_limit.allow(client, Instant::now()) {
        return rate_limited_response("Too many session requests. Wait a minute and try again.");
    }
    // One record, so the three values cannot come from different providers.
    let (room_name, provider) = match room_and_available_provider(&state).await {
        ProviderChoice::Ready(room_name, provider) => (room_name, provider),
        ProviderChoice::NoneConfigured => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({
                    "error": "Server is missing LiveKit credentials. Create config/codetrial.env.local or set LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET."
                }),
            );
        }

        // Naming the cause, because the browser cannot. A refused upgrade
        // reaches the page as a bare socket error with no status attached, so a
        // candidate told only "could not connect" would go looking at their own
        // network for a quota this server already knows is spent.
        ProviderChoice::AllExhausted => {
            return json_response(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({
                    "code": "livekit_quota_exhausted",
                    "error": "Every configured LiveKit project is out of connection minutes. Ask the operator to top one up."
                }),
            );
        }
    };

    let Ok(body) = to_bytes(request.into_body(), MAX_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };

    // Before the token is minted, and therefore before any Egress call can be
    // made for this room. A recorded interview with no persisted consent is the
    // one state this whole feature exists to make impossible. Ahead of the
    // consent check, because that check reads the body too and answers 403 to
    // anything it cannot parse. A malformed body is a client error whether or
    // not this server records, and turning it into "you did not consent" sends
    // the candidate looking for a checkbox they already ticked.
    if !body.is_empty() && serde_json::from_slice::<Value>(body.as_ref()).is_err() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Session request must be JSON." }),
        );
    }
    let interview = match checked_consent(&state, &accounts, &user, body.as_ref()).await {
        Ok(interview) => interview,
        Err(response) => return response,
    };
    let identity = generated_candidate_identity();
    let response = match token_response(
        &TokenConfig {
            api_key: &provider.api_key,
            api_secret: &provider.api_secret,
            server_url: &provider.url,
            recording_max_min: state.config.recording.as_ref().map(|it| it.max_minutes),
        },
        body.as_ref(),
        &room_name,
        &identity,
        current_epoch_seconds(),
    ) {
        Ok(response) => response,

        // A body that would not parse is the caller's fault, not ours. The
        // status comes from the one parse inside `token_response` rather than
        // from a second copy of the same check out here.
        Err(error) if error.is::<serde_json::Error>() => {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "Session request must be JSON." }),
            );
        }
        Err(error) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": error.to_string() }),
            );
        }
    };

    // Before the dispatcher, so two requests racing on one interview cannot
    // both start an interviewer, and after the token, so a signing failure
    // costs nothing. The loser of that race is refused here without dispatching
    // anything.
    if let Some(interview) = &interview
        && let Err(response) =
            claim_consent(&accounts, user.id, interview.clone(), &room_name).await
    {
        return response;
    }

    // Validate the request before starting anything external. Otherwise a
    // malformed body can leave an interviewer running for a request that got a
    // 400 response.
    if let Some(dispatcher) = &state.dispatcher
        && !dispatcher.ensure_agent(&room_name, provider)
    {
        // The claim above is given back first. A busy server tells the
        // candidate to retry, and a retry refused because the first attempt
        // spent their consent is worse than the refusal it followed.
        if let Some(interview) = &interview {
            let accounts = accounts.clone();
            let interview = interview.clone();
            let room_name = room_name.clone();
            let owner = user.id;
            if let Err(error) =
                blocking(move || release_interview_room(&accounts, &interview, owner, &room_name))
                    .await
            {
                eprintln!("could not release interview consent after a refused dispatch: {error}");
            }
        }

        // Refusing is the honest failure. Handing out the token anyway would
        // put the candidate in an empty room reading "Waiting" with nothing, on
        // screen or in any log they can see, saying why.
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "error": "The server is running as many interviews as it can right now. Try again in a few minutes."
            }),
        );
    }

    state
        .room_authorizations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .grant(room_name, user.id, current_epoch_seconds());

    json_response(
        StatusCode::OK,
        json!({
            "token": response.token,
            "serverUrl": response.server_url,
            "roomName": response.room_name
        }),
    )
}

pub(crate) async fn observer_token_handler(
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    let user = match current_user(state.accounts.as_ref(), request.headers()).await {
        Ok(Some(user)) => user,
        Ok(None) => return unauthorized_response(),
        Err(_) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read account session." }),
            );
        }
    };
    let Ok(body) = to_bytes(request.into_body(), MAX_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Some(room_name) = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|body| body.get("roomName")?.as_str().map(str::to_owned))
    else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Observer requests need a room name." }),
        );
    };
    let now = current_epoch_seconds();
    let authorized = state
        .room_authorizations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .allows(&room_name, user.id, now);
    if !authorized {
        return json_response(
            StatusCode::FORBIDDEN,
            json!({ "error": "You are not allowed to observe this room." }),
        );
    }
    let Some(provider) = state
        .config
        .pool
        .for_room(&room_name, &state.config.room_prefix)
    else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Server is missing LiveKit credentials." }),
        );
    };
    let identity = format!("observer-{}", suffix(12));
    match livekit_observer_token(
        &provider.api_key,
        &provider.api_secret,
        &identity,
        &room_name,
        now,
    ) {
        Ok(token) => json_response(
            StatusCode::OK,
            json!({ "token": token, "serverUrl": provider.url, "roomName": room_name }),
        ),
        Err(error) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": error.to_string() }),
        ),
    }
}

/// Room-name suffix. Random, not clock-derived.
///
/// Room names are not secrets, so the old `SystemTime::now().as_nanos()` was
/// not an exploit. It was the wrong tool for a namespace that must not collide,
/// and it read as random to anyone who did not look. Two tokens minted inside
/// the same nanosecond tick produced the same suffix.
///
/// Falls back to the clock only if the OS entropy source fails, which is the
/// same behaviour as before rather than a panic in the middle of a request.
pub(crate) fn suffix(len: usize) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut bytes = vec![0u8; len];
    if ring::rand::SystemRandom::new().fill(&mut bytes).is_err() {
        let mut value = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for byte in &mut bytes {
            *byte = (value % 256) as u8;
            value /= 256;
        }
    }
    bytes
        .iter()
        .map(|byte| DIGITS[(*byte as usize) % DIGITS.len()] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderValue, header};

    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([127, 0, 0, last])
    }

    /// An observer grant belongs to one account and dies with the token.
    ///
    /// This decides who may watch a live interview, so every edge of it
    /// matters: the moment a grant stops counting, the account it was issued
    /// to, and the sweep that clears the map without ever clearing a grant
    /// that is still good.
    #[test]
    fn a_room_grant_expires_and_belongs_to_one_account() {
        let mut grants = RoomAuthorizations::default();
        grants.grant("interview-alice".to_string(), 7, 1_000);

        assert!(grants.allows("interview-alice", 7, 1_000));
        assert!(
            !grants.allows("interview-alice", 8, 1_000),
            "a grant is to an account, not to whoever knows the room name"
        );
        assert!(!grants.allows("interview-bob", 7, 1_000), "and to one room");

        // The last second it is good, and the first it is not. Expiry is not a
        // sweep: a grant nobody has swept yet is still over.
        assert!(grants.allows("interview-alice", 7, 1_000 + TOKEN_TTL_SECONDS - 1));
        assert!(!grants.allows("interview-alice", 7, 1_000 + TOKEN_TTL_SECONDS));

        // The sweep is what bounds the map, and `allows` answers expiry on its
        // own, so nothing about who may watch can show whether it ran. Counted
        // here instead: a server that never sweeps keeps every room it ever
        // issued a token for, for as long as it runs.
        let mut swept = RoomAuthorizations::default();
        swept.grant("interview-old".to_string(), 1, 0);
        swept.grant("interview-later".to_string(), 2, TOKEN_TTL_SECONDS * 2);
        assert_eq!(
            swept.grants.len(),
            1,
            "a grant expired long ago is not still held"
        );

        // A grant expiring exactly now is over, and goes with the rest.
        let mut boundary = RoomAuthorizations::default();
        boundary.grant("interview-edge".to_string(), 1, 0);
        boundary.grant("interview-trigger".to_string(), 2, TOKEN_TTL_SECONDS);
        assert_eq!(
            boundary.grants.len(),
            1,
            "expiring on the second of the sweep is still expiring"
        );

        // And a live grant survives the sweep, or watching stops mid-interview.
        let mut live = RoomAuthorizations::default();
        live.grant("interview-new".to_string(), 2, 100);
        live.grant("interview-trigger".to_string(), 3, TOKEN_TTL_SECONDS + 50);
        assert!(
            live.allows("interview-new", 2, TOKEN_TTL_SECONDS + 50),
            "a grant still inside its lifetime is not swept out from under it"
        );
    }

    fn forwarded(value: &str) -> header::HeaderMap {
        let mut headers = header::HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_str(value).unwrap());
        headers
    }

    fn peer() -> SocketAddr {
        SocketAddr::from(([10, 0, 0, 9], 4000))
    }

    #[test]
    fn generated_candidate_identity_has_the_livekit_prefix() {
        let generated = generated_candidate_identity();
        assert!(generated.starts_with("candidate-"));
        assert_eq!(generated.len(), "candidate-".len() + CANDIDATE_SUFFIX_LEN);
    }

    #[test]
    fn client_ip_ignores_forwarded_headers_until_a_proxy_is_declared() {
        // The default must not let a direct caller mint a new identity per
        // request and walk straight past the rate limit.
        assert_eq!(
            client_ip(&forwarded("1.2.3.4"), peer(), 0),
            peer().ip(),
            "a spoofed header must be ignored when no proxy is configured"
        );
        assert_eq!(
            client_ip(&header::HeaderMap::new(), peer(), 1),
            peer().ip(),
            "a configured proxy that sent no header falls back to the peer"
        );
    }

    /// Expiry is enforced on the entry that is read, not by pruning first, so
    /// a grant the sweep has not reached yet must still be refused. The sweep
    /// runs at most once per token lifetime, which is exactly long enough for
    /// an expired grant to still be sitting in the map.
    #[test]
    fn an_observation_grant_expires_whether_or_not_the_sweep_has_run() {
        let mut rooms = RoomAuthorizations::default();
        rooms.grant("interview-one".to_string(), 7, 1_000);

        assert!(rooms.allows("interview-one", 7, 1_000));
        assert!(
            !rooms.allows("interview-one", 8, 1_000),
            "a grant belongs to the account it was made for"
        );
        assert!(!rooms.allows("interview-two", 7, 1_000));

        // One second past the token's life, and no second grant has run, so
        // nothing has swept.
        let expired = 1_000 + TOKEN_TTL_SECONDS + 1;
        assert!(!rooms.allows("interview-one", 7, expired));

        // A later grant sweeps the dead entry out rather than letting the map
        // grow for the life of the process.
        rooms.grant("interview-three".to_string(), 7, expired);
        assert_eq!(rooms.grants.len(), 1);
        assert!(rooms.allows("interview-three", 7, expired));
    }

    #[test]
    fn client_ip_reads_the_hop_the_operator_declared() {
        // Each proxy appends the address it saw, so the client sits `hops` from
        // the right; anything further left is caller-controlled.
        let chain = forwarded("9.9.9.9, 1.2.3.4, 172.16.0.1");
        assert_eq!(client_ip(&chain, peer(), 1), IpAddr::from([172, 16, 0, 1]));
        assert_eq!(client_ip(&chain, peer(), 2), IpAddr::from([1, 2, 3, 4]));
        assert_eq!(client_ip(&chain, peer(), 3), IpAddr::from([9, 9, 9, 9]));
        assert_eq!(
            client_ip(&chain, peer(), 4),
            peer().ip(),
            "more hops than entries means the chain is not what was declared"
        );
    }

    #[test]
    fn client_ip_handles_ports_and_ipv6() {
        assert_eq!(
            client_ip(&forwarded("1.2.3.4:5678"), peer(), 1),
            IpAddr::from([1, 2, 3, 4])
        );
        assert_eq!(
            client_ip(&forwarded("2001:db8::1"), peer(), 1),
            "2001:db8::1".parse::<IpAddr>().unwrap(),
            "IPv6 colons must not be mistaken for a port"
        );
        assert_eq!(
            client_ip(&forwarded("[2001:db8::1]:443"), peer(), 1),
            "2001:db8::1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            client_ip(&forwarded("not-an-address"), peer(), 1),
            peer().ip()
        );
    }

    #[test]
    fn rate_limit_window_expires_so_a_blocked_client_recovers() {
        let limit = TokenRateLimit::with_limit(TOKEN_RATE_LIMIT);
        let start = Instant::now();

        for attempt in 1..=TOKEN_RATE_LIMIT {
            assert!(limit.allow(ip(1), start), "attempt {attempt} should pass");
        }
        assert!(!limit.allow(ip(1), start), "the budget is spent");

        // Still inside the window: hammering must not extend the block past it.
        let late = start + TOKEN_RATE_WINDOW - Duration::from_secs(1);
        assert!(!limit.allow(ip(1), late));

        // A blocked client has to recover, or one burst bans them forever.
        assert!(limit.allow(ip(1), start + TOKEN_RATE_WINDOW));
    }

    /// The sweep runs at most once per window, so an entry can outlive its own
    /// window while no sweep is due. Without the per-entry reset that client
    /// stays blocked until the next sweep happens to come round.
    #[test]
    fn a_window_expires_even_when_no_sweep_is_due() {
        let limit = TokenRateLimit::with_limit(TOKEN_RATE_LIMIT);
        let start = Instant::now();

        // Spend one client's budget early, then force a sweep it survives, so
        // the next sweep is a full window away.
        for _ in 0..TOKEN_RATE_LIMIT {
            assert!(limit.allow(ip(1), start + Duration::from_secs(50)));
        }
        assert!(!limit.allow(ip(1), start + Duration::from_secs(50)));
        assert!(limit.allow(ip(2), start + TOKEN_RATE_WINDOW));
        assert_eq!(
            limit.state.lock().unwrap().windows.len(),
            2,
            "the blocked client is still inside its window and must survive the sweep"
        );

        // 65s after it started: its window is over, but only 55s since the
        // sweep, so no sweep is due.
        assert!(
            limit.allow(ip(1), start + Duration::from_secs(115)),
            "an expired window must reset itself rather than wait for a sweep"
        );
    }

    #[test]
    fn rate_limit_is_per_client_and_prunes_expired_windows() {
        let limit = TokenRateLimit::with_limit(TOKEN_RATE_LIMIT);
        let start = Instant::now();

        for _ in 0..TOKEN_RATE_LIMIT {
            assert!(limit.allow(ip(1), start));
        }
        assert!(!limit.allow(ip(1), start), "first client is spent");
        assert!(
            limit.allow(ip(2), start),
            "a second client must not inherit the first client's budget"
        );

        // The map is only bounded by pruning, so an idle client must not
        // linger.
        assert_eq!(limit.state.lock().unwrap().windows.len(), 2);
        assert!(limit.allow(ip(3), start + TOKEN_RATE_WINDOW));
        assert_eq!(
            limit.state.lock().unwrap().windows.len(),
            1,
            "expired windows should be dropped, not accumulated"
        );
    }
}

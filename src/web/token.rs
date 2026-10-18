//! Minting the LiveKit tokens a page joins a room with, for candidates and
//! observers alike, plus the per-client rate limit that decides who gets to
//! ask. Distinct from `crate::token`, which does the signing.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use ring::rand::SecureRandom;
use serde_json::{Value, json};

use crate::config::{DEFAULT_DURATION_MIN, MAX_DURATION_MIN, MIN_DURATION_MIN};
use crate::current_epoch_seconds;
use crate::token::{LivekitTokenInput, TOKEN_TTL_SECONDS, livekit_observer_token, livekit_token};

use super::auth::{Owner, current_user};
use super::consent::{checked_consent, claim_consent};
use super::pool::{ProviderChoice, room_and_available_provider};
use super::{
    AppState, MAX_BODY_BYTES, NoShowConsent, client_ip, json_response, number_json,
    rate_limited_response, unauthorized_response,
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
///
/// No `Eq`, because `duration_min` is a JSON number and one of those can be a
/// float. Nothing compares these, and the alternative is rounding the value
/// that has to reach the page exactly as the agent reads it.
#[derive(Clone, PartialEq)]
pub struct TokenResponse {
    pub token: String,
    pub server_url: String,
    pub room_name: String,
    /// The length this interview actually runs for, which is not always the
    /// length that was asked for: `token_duration_min` clamps to the range and,
    /// where this server records, to the recording cap. The page runs its own
    /// countdown and its own round split, so it has to be told the answer
    /// rather than keep the question it asked.
    ///
    /// Read back out of the metadata with the agent's own
    /// `duration_from_metadata`, rather than taken from the value that went in.
    /// The metadata keeps a fractional request fractional and the agent
    /// truncates it to whole minutes before it builds the deadline, so handing
    /// the page the value as written would have it counting 42.8 minutes
    /// against an interview the interviewer ends at 42. One function, so the
    /// two cannot answer differently.
    pub duration_min: Value,
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

/// A partial upload must not hold a token request open forever. This deadline
/// ends before any room is reserved, consent is claimed, or agent is started.
const TOKEN_BODY_READ_LIMIT: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq, Eq)]
enum TokenBodyReadError {
    TimedOut,
    Rejected,
}

async fn read_token_body(body: Body, limit: Duration) -> Result<Bytes, TokenBodyReadError> {
    match tokio::time::timeout(limit, to_bytes(body, MAX_BODY_BYTES)).await {
        Err(_) => Err(TokenBodyReadError::TimedOut),
        Ok(Err(_)) => Err(TokenBodyReadError::Rejected),
        Ok(Ok(body)) => Ok(body),
    }
}

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

fn interviewer_choices() -> [(&'static str, &'static [&'static str]); 2] {
    [
        ("interviewerVoice", crate::agent::INTERVIEWER_VOICES),
        ("interviewerAccent", crate::agent::INTERVIEWER_ACCENTS),
    ]
}

fn validate_interviewer_selections(request: &Value) -> Result<(), String> {
    for (field, choices) in interviewer_choices() {
        match request.get(field) {
            None => {}
            Some(Value::String(value))
                if value.is_empty()
                    || value == "Default"
                    || value == "Random"
                    || choices.contains(&value.as_str()) => {}
            _ => return Err(format!("Unsupported {field} selection.")),
        }
    }
    Ok(())
}

fn resolve_interviewer_choice(
    value: Option<&Value>,
    choices: &[&'static str],
    mut draw: impl FnMut() -> Result<u8, &'static str>,
) -> Result<Option<&'static str>, Box<dyn std::error::Error + Send + Sync>> {
    if value.and_then(Value::as_str) != Some("Random") {
        return Ok(crate::agent::sanitize_interviewer_choice(value, choices));
    }

    // Reject the incomplete bucket so every named option has equal weight.
    // Resolve here only: the signed metadata carries a concrete choice through
    // participant reconnects and every subsequent Gemini setup.
    let limit = 256 - 256 % choices.len();
    loop {
        let byte = usize::from(draw()?);
        if byte < limit {
            return Ok(Some(choices[byte % choices.len()]));
        }
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
    let interview_mode =
        crate::agent::InterviewMode::parse(request.get("interviewMode").and_then(Value::as_str));
    let profile = crate::agent::sanitize_interview_profile(request.get("interviewProfile"));
    let grounding = crate::agent::sanitize_interview_grounding(request.get("interviewGrounding"));
    let mut metadata = json!({
        "problemId": problem_id,
        "durationMin": duration_min.clone(),
        "interviewLoop": interview_loop.as_str(),
        "interviewMode": interview_mode.as_str(),
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

    // Only a literal true hides them, and an absent key means shown, so a
    // session that never ticked the box mints the same metadata as before.
    if request.get("hideExamples") == Some(&Value::Bool(true)) {
        metadata
            .as_object_mut()
            .expect("metadata is an object")
            .insert("hideExamples".to_string(), Value::Bool(true));
    }
    if !interview_mode.is_whiteboard() && request.get("exampleDrawing") == Some(&Value::Bool(true))
    {
        metadata
            .as_object_mut()
            .expect("metadata is an object")
            .insert("exampleDrawing".to_string(), Value::Bool(true));
    }
    if request.get("codeExecution") == Some(&Value::Bool(false)) {
        metadata
            .as_object_mut()
            .expect("metadata is an object")
            .insert("codeExecution".to_string(), Value::Bool(false));
    }
    for (field, choices) in interviewer_choices() {
        if let Some(choice) = resolve_interviewer_choice(request.get(field), choices, || {
            let mut byte = [0u8];
            ring::rand::SystemRandom::new()
                .fill(&mut byte)
                .map_err(|_| "Could not select an interviewer style.")?;
            Ok(byte[0])
        })? {
            metadata[field] = json!(choice);
        }
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
        duration_min: json!(crate::agent::duration_from_metadata(Some(&duration_min))),
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

/// A room on a provider that still has minutes, or the refusal to send back.
///
/// The two failures are one function because they are one question with two
/// answers a candidate cannot tell apart: no provider is a server the operator
/// never finished setting up, and every provider spent is a server that worked
/// until this morning.
#[allow(clippy::result_large_err)] // Responses are immediately returned by HTTP handlers.
pub(crate) async fn reserved_room(
    state: &AppState,
    require_fresh_room: bool,
) -> Result<(String, &crate::config::Provider), Response> {
    match room_and_available_provider(state, require_fresh_room).await {
        ProviderChoice::Ready(room_name, provider) => Ok((room_name, provider)),
        ProviderChoice::NoneConfigured => Err(json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "error": "Server is missing LiveKit credentials. Create config/codetrial.env.local or set LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET."
            }),
        )),

        // Naming the cause, because the browser cannot. A refused upgrade
        // reaches the page as a bare socket error with no status attached, so a
        // candidate told only "could not connect" would go looking at their own
        // network for a quota this server already knows is spent.
        ProviderChoice::AllUnavailable(unavailable) => {
            let code = super::pool::ProviderVerdict::worst(
                unavailable.iter().map(|(_, verdict)| *verdict),
            )
            .code();
            let detail = super::pool::unavailable_projects(&unavailable).join(", ");
            Err(json_response(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({
                    "code": code,
                    "error": format!(
                        "Every configured LiveKit project is unavailable ({detail}). Ask the operator to restore a credential or connection minutes."
                    )
                }),
            ))
        }
    }
}

/// The caller admitted to request an interview token.
enum AdmittedCaller {
    Guest,
    Owner(Owner),
}

/// Every gate that can refuse a request before any external work happens.
///
/// One function rather than four checks spread down the handler, because the
/// order between them is the property worth protecting and it is invisible
/// when they are apart. Account and verified-email checks run before the rate
/// limit, so requests that cannot be admitted do not consume token capacity.
///
/// Guests and authenticated callers use separate IP-based token limits,
/// so anonymous traffic cannot spend a signed-in candidate's allowance.
/// Guests are admitted only when recording is disabled.
#[allow(clippy::result_large_err)]
async fn admit_caller(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    peer: SocketAddr,
) -> Result<AdmittedCaller, Response> {
    let user = match current_user(state.accounts.as_ref(), headers).await {
        Ok(user) => user,
        Err(_) => {
            return Err(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read account session." }),
            ));
        }
    };

    let client = client_ip(headers, peer, state.config.trusted_proxy_hops);
    let now = Instant::now();

    let Some(user) = user else {
        if !state.config.guest_mode_enabled {
            return Err(json_response(
                StatusCode::UNAUTHORIZED,
                json!({ "error": "Enter your GitHub username to start an interview." }),
            ));
        }

        // A configured but unavailable account database must fail closed.
        if state.accounts_required && state.accounts.is_none() {
            return Err(state.accounts_error());
        }

        if state.config.recording.is_some() {
            return Err(json_response(
                StatusCode::UNAUTHORIZED,
                json!({ "error": "Sign in with GitHub to start a recorded interview." }),
            ));
        }

        if !state.guest_mode_available() {
            return Err(json_response(
                StatusCode::FORBIDDEN,
                json!({ "error": "Guest interviews are disabled." }),
            ));
        }

        if !state.guest_token_limit.allow(client, now) {
            return Err(rate_limited_response(
                "Too many session requests. Wait a minute and try again.",
            ));
        }

        return Ok(AdmittedCaller::Guest);
    };

    let Some(accounts) = state.accounts.clone() else {
        return Err(state.accounts_error());
    };

    if state.config.recording.is_some() && user.verified_email.is_none() {
        return Err(json_response(
            StatusCode::FORBIDDEN,
            json!({
                "code": "recording_requires_verified_identity",
                "error": "This interview is recorded, so it needs a GitHub sign-in. A typed username cannot receive the recording."
            }),
        ));
    }

    if !state.token_limit.allow(client, now) {
        return Err(rate_limited_response(
            "Too many session requests. Wait a minute and try again.",
        ));
    }

    Ok(AdmittedCaller::Owner(Owner { accounts, user }))
}
/// Release recorded interview consent when dispatch refuses a room.
/// Guests have no persisted consent to release.
async fn dispatch_or_release_consent(
    state: &AppState,
    owner: Option<&Owner>,
    interview: Option<&String>,
    room_name: &str,
    provider: &crate::config::Provider,
) -> Option<Response> {
    let Some(dispatcher) = &state.dispatcher else {
        return None;
    };

    let no_show = owner.zip(interview).map(|(owner, interview)| {
        NoShowConsent::new(
            owner.accounts.clone(),
            interview.clone(),
            owner.user.id,
            room_name.to_string(),
        )
    });

    let dispatch_result = if owner.is_none() {
        dispatcher.ensure_guest_agent(room_name, provider)
    } else {
        dispatcher.ensure_agent_with_no_show_release(room_name, provider, no_show.clone())
    };

    let Err(refusal) = dispatch_result else {
        return None;
    };

    if let Some(no_show) = no_show {
        match no_show.release().await {
            Ok(true) => {}
            Ok(false) => eprintln!(
                "codetrial consent_release_skipped room={room_name} \
                 reason=claim_changed_or_pending_interview"
            ),
            Err(error) => {
                eprintln!("could not release interview consent after a refused dispatch: {error}")
            }
        }
    }

    let error = match refusal {
        super::DispatchRefusal::AtCapacity => {
            "The server is running as many interviews as it can right now. Try again in a few minutes."
        }
        super::DispatchRefusal::Finalizing => {
            "This room is still finishing the previous interview's report. Try again in a few minutes."
        }
    };

    Some(json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({ "error": error }),
    ))
}

/// Keeps the consent claim and its dispatch in one task if the client drops
/// the request while the blocking database claim is still finishing.
async fn claim_and_dispatch(
    state: AppState,
    owner: Option<Owner>,
    interview: Option<String>,
    room_name: String,
    provider: crate::config::Provider,
) -> Option<Response> {
    if let (Some(owner), Some(interview)) = (owner.as_ref(), interview.as_ref())
        && let Err(response) = claim_consent(
            &owner.accounts,
            owner.user.id,
            interview.clone(),
            &room_name,
        )
        .await
    {
        return Some(response);
    }

    dispatch_or_release_consent(
        &state,
        owner.as_ref(),
        interview.as_ref(),
        &room_name,
        &provider,
    )
    .await
}

fn minted_token(
    state: &AppState,
    provider: &crate::config::Provider,
    body: &[u8],
    room_name: &str,
    identity: &str,
) -> Result<TokenResponse, String> {
    match token_response(
        &TokenConfig {
            api_key: &provider.api_key,
            api_secret: &provider.api_secret,
            server_url: &provider.url,
            recording_max_min: state.config.recording.as_ref().map(|it| it.max_minutes),
        },
        body,
        room_name,
        identity,
        current_epoch_seconds(),
    ) {
        Ok(response) => Ok(response),

        // No arm for a parse failure. `token_response` parses only a body that
        // is not empty, and `token_handler` has already refused a non-empty
        // body that will not parse, with the same call and the same 400. An arm
        // here could only answer a request that cannot reach it, which is why
        // both of its mutants survived the gate: nothing distinguishes a guard
        // that is never true from one that is never false.
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) async fn token_handler(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response {
    let owner = match admit_caller(&state, request.headers(), peer).await {
        Ok(AdmittedCaller::Guest) => None,
        Ok(AdmittedCaller::Owner(owner)) => Some(owner),
        Err(response) => return response,
    };

    let body = match read_token_body(request.into_body(), TOKEN_BODY_READ_LIMIT).await {
        Ok(body) => body,
        Err(TokenBodyReadError::TimedOut) => {
            return json_response(
                StatusCode::REQUEST_TIMEOUT,
                json!({ "error": "Session request body timed out." }),
            );
        }
        Err(TokenBodyReadError::Rejected) => {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
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
    let mut request = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice::<Value>(&body).expect("JSON was checked above")
    };
    if let Err(error) = validate_interviewer_selections(&request) {
        return json_response(StatusCode::BAD_REQUEST, json!({ "error": error }));
    }

    let interview = if let Some(owner) = owner.as_ref() {
        match checked_consent(&state, &owner.accounts, &owner.user, body.as_ref()).await {
            Ok(interview) => interview,
            Err(response) => return response,
        }
    } else {
        // Guests are admitted only when recording is disabled.
        None
    };

    let assignment = if let Some(owner) = owner.as_ref() {
        match super::selection::assign(
            &request,
            owner.user.id,
            &owner.accounts.config().session_secret,
        ) {
            Ok(assignment) => assignment,
            Err(error) => return error.response(),
        }
    } else {
        // Strict receipts authenticate an account's preceding assignment.
        // Practice guests need neither an account nor a receipt secret.
        match request.get("selectionMode") {
            None => None,
            Some(Value::String(mode)) if mode == "practice" => None,
            Some(Value::String(mode)) if mode == "strict" => {
                return json_response(
                    StatusCode::UNAUTHORIZED,
                    json!({
                        "code": "strict_login_required",
                        "error": "Sign in to start Strict Interview. Guest interviews use Practice."
                    }),
                );
            }
            _ => return super::selection::SelectionError::Mode.response(),
        }
    };

    // Strict starts a new attempt even on a server with a pinned development
    // room. Reusing that room would reuse its interviewer and evidence.
    let (room_name, provider) = match reserved_room(&state, assignment.is_some()).await {
        Ok(reserved) => reserved,
        Err(response) => return response,
    };
    let body = if let Some(assignment) = &assignment {
        request["problemId"] = json!(assignment.page);
        serde_json::to_vec(&request).expect("JSON value is serializable")
    } else {
        body.to_vec()
    };
    let identity = generated_candidate_identity();
    let response = match minted_token(&state, provider, body.as_ref(), &room_name, &identity) {
        Ok(response) => response,
        Err(error) => {
            return json_response(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": error }));
        }
    };

    // Before the dispatcher, so two requests racing on one interview cannot
    // both start an interviewer, and after the token, so a signing failure
    // costs nothing. Spawned as one operation because dropping a request must
    // not cancel the handler after the blocking claim commits but before the
    // matching dispatcher sees it. Dropping a Tokio join handle detaches its
    // task, so this pair finishes even when the browser has gone away.
    let startup = tokio::spawn(claim_and_dispatch(
        state.clone(),
        owner.as_ref().map(|owner| Owner {
            accounts: Arc::clone(&owner.accounts),
            user: owner.user.clone(),
        }),
        interview.clone(),
        room_name.clone(),
        provider.clone(),
    ));
    match startup.await {
        Ok(Some(refusal)) => return refusal,
        Ok(None) => {}
        Err(error) => {
            eprintln!("could not finish the consent claim and room dispatch: {error}");
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not start the interview." }),
            );
        }
    }

    if let Some(owner) = owner.as_ref() {
        state
            .room_authorizations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .grant(room_name, owner.user.id, current_epoch_seconds());
    }

    let mut body = json!({
        "token": response.token,
        "serverUrl": response.server_url,
        "roomName": response.room_name,
        "guest": owner.is_none(),
        // The same granted length the agent reads from its token metadata.
        "durationMin": response.duration_min
    });
    if let Some(assignment) = assignment {
        body["problemPage"] = json!(assignment.page);
        body["previousAssignment"] = json!(assignment.receipt);
    }
    json_response(StatusCode::OK, body)
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
#[path = "../../tests/unit/web/token.rs"]
mod tests;

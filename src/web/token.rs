//! Minting the LiveKit tokens a page joins a room with, for candidates and
//! observers alike, plus the per-client rate limit that decides who gets to
//! ask. Distinct from `crate::token`, which does the signing.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::Ordering;
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
use super::{
    AppState, MAX_BODY_BYTES, WebServerConfig, client_ip, json_response, number_json,
    rate_limited_response, unauthorized_response,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenConfig<'a> {
    pub api_key: &'a str,
    pub api_secret: &'a str,
    pub server_url: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Clone, Default)]
pub(crate) struct TokenRateLimit(Arc<Mutex<RateLimitState>>);

pub(crate) struct RateLimitState {
    windows: HashMap<IpAddr, Window>,
    last_sweep: Instant,
}

impl Default for RateLimitState {
    fn default() -> Self {
        Self {
            windows: HashMap::new(),
            last_sweep: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Window {
    started: Instant,
    count: u32,
}

impl TokenRateLimit {
    /// Crosses a module boundary: the token handlers and the login handler in
    /// `super::auth` each hold their own limiter of this type.
    pub(crate) fn allow(&self, client: IpAddr, now: Instant) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());

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
        window.count <= TOKEN_RATE_LIMIT
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RoomAuthorization {
    owner_id: i64,
    expires_at: u64,
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
    let duration_min = token_duration_min(request.get("durationMin"));
    let metadata = json!({
        "problemId": problem_id,
        "durationMin": duration_min,
        "candidateIdentity": default_identity,
    })
    .to_string();

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

pub(crate) fn token_duration_min(value: Option<&Value>) -> Value {
    let duration = value.and_then(crate::agent::json_number).unwrap_or(0.0);
    if duration == 0.0 || duration.is_nan() {
        json!(DEFAULT_DURATION_MIN)
    } else {
        // Fractional minutes stay fractional: the metadata is echoed back to
        // the frontend, which renders whatever the lobby asked for.
        number_json(duration.clamp(MIN_DURATION_MIN.into(), MAX_DURATION_MIN.into()))
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
        .insert(
            room_name,
            RoomAuthorization {
                owner_id: user.id,
                expires_at: current_epoch_seconds() + TOKEN_TTL_SECONDS,
            },
        );

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
    let authorized = {
        let mut rooms = state
            .room_authorizations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        rooms.retain(|_, grant| grant.expires_at > now);
        rooms
            .get(&room_name)
            .is_some_and(|grant| grant.owner_id == user.id)
    };
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

/// How long a project's quota verdict is trusted.
///
/// LiveKit Cloud bills connection minutes against a period, so a project that
/// has run out stays out for the rest of it, and one with room to spare does
/// not fill up inside a minute. Long enough that a burst of interview starts
/// costs one probe, short enough that topping a project up brings it back
/// without a restart.
const PROVIDER_QUOTA_TTL: Duration = Duration::from_secs(60);

/// How long the probe itself may take, which is not the shared client's
/// 30-second backstop.
///
/// This runs while a candidate waits on `/api/token`, and its answer is only an
/// optimization: a project that does not reply promptly is treated as available
/// and tried, which is what would have happened without any probe at all.
/// Waiting half a minute to learn something optional is the worse trade.
const PROVIDER_QUOTA_PROBE_TIMEOUT: Duration = Duration::from_secs(1);

/// What each project last said about its own quota, so a run of interview
/// starts does not re-ask a project that just answered.
///
/// Keyed by provider id rather than URL, because the id is what the room name
/// carries and therefore what the agent resolves the same project from.
#[derive(Clone, Default)]
pub(crate) struct ProviderQuota(Arc<Mutex<HashMap<String, (bool, Instant)>>>);

impl ProviderQuota {
    /// Whether this project can still take a connection, from cache where the
    /// last answer is recent enough and from the project itself otherwise.
    pub(crate) async fn available(&self, provider: &crate::config::Provider) -> bool {
        let now = Instant::now();
        {
            let cache = self.0.lock().unwrap_or_else(|error| error.into_inner());
            if let Some((verdict, at)) = cache.get(&provider.id)
                && now.duration_since(*at) < PROVIDER_QUOTA_TTL
            {
                return *verdict;
            }
        }
        let verdict = has_connection_minutes(provider).await;
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(provider.id.clone(), (verdict, Instant::now()));
        verdict
    }
}

/// Whether a project still has connection minutes.
///
/// `/rtc/validate` answers this without opening a socket, creating a room or
/// spending a minute of the quota it is asking about: 200 where the project has
/// room, 429 where it does not. The room management API is not a substitute --
/// it answers 200 on an exhausted project, because the limit is on media
/// connections rather than on the Twirp surface.
///
/// Only an explicit 429 takes a project out. A probe that cannot be sent at all
/// says nothing about quota, and refusing every interview because this server
/// briefly lost the network would be a worse failure than the one being
/// avoided: the candidate would be turned away from a project that works.
async fn has_connection_minutes(provider: &crate::config::Provider) -> bool {
    let Some(origin) = super::policy::livekit_http_origin(&provider.url) else {
        return true;
    };
    let Ok(token) = livekit_token(LivekitTokenInput {
        api_key: &provider.api_key,
        api_secret: &provider.api_secret,
        name: "quota-probe",
        identity: "quota-probe",
        room: "quota-probe",
        metadata: "",
        now_seconds: crate::current_epoch_seconds(),
        agent: false,
    }) else {
        return true;
    };
    let response = crate::http_client()
        .get(format!("{origin}/rtc/validate"))
        .bearer_auth(token)
        .timeout(PROVIDER_QUOTA_PROBE_TIMEOUT)
        .send()
        .await;
    !matches!(response, Ok(response) if response.status() == StatusCode::TOO_MANY_REQUESTS)
}

/// Which project this interview runs on, skipping any that has run out.
pub(crate) enum ProviderChoice<'a> {
    Ready(String, &'a crate::config::Provider),
    NoneConfigured,
    AllExhausted,
}

/// Round-robin as before, but a project that answers "out of minutes" is passed
/// over rather than handed to the candidate.
///
/// The counter advances per attempt, not per request, so a dead project does
/// not pin every later interview to the one after it.
pub(crate) async fn room_and_available_provider(state: &AppState) -> ProviderChoice<'_> {
    // A pinned development room names the project it belongs to, and moving the
    // candidate off it would defeat the pin.
    if !state.config.production
        && state
            .config
            .fixed_room_name
            .as_deref()
            .is_some_and(|name| !name.is_empty())
    {
        let counter = state.provider_counter.fetch_add(1, Ordering::Relaxed);
        return match room_and_provider(&state.config, counter) {
            (room_name, Some(provider)) => ProviderChoice::Ready(room_name, provider),
            (_, None) => ProviderChoice::NoneConfigured,
        };
    }

    if state.config.pool.providers.is_empty() {
        return ProviderChoice::NoneConfigured;
    }
    for _ in 0..state.config.pool.providers.len() {
        // Bumped per attempt rather than once per request: adding the attempt
        // number to one shared value locally would leave the next request
        // starting at the exhausted project again, so the project immediately
        // after a dead one absorbs its share as well as its own.
        let counter = state.provider_counter.fetch_add(1, Ordering::Relaxed);
        let (room_name, provider) = room_and_provider(&state.config, counter);
        let Some(provider) = provider else {
            return ProviderChoice::NoneConfigured;
        };
        if state.provider_quota.available(provider).await {
            return ProviderChoice::Ready(room_name, provider);
        }
        eprintln!(
            "livekit provider {} is out of connection minutes; trying the next project",
            provider.id
        );
    }
    ProviderChoice::AllExhausted
}

/// Picks the provider first and writes its id into the room name, because the
/// agent process is launched separately and the room name is the only thing it
/// is handed. Recomputing the choice on the other side, from a pool assembled
/// by a second directory scan, is how a candidate ends up holding a token for
/// one LiveKit project while the agent waits in another.
///
/// The primary contributes no segment, so a single-provider deployment keeps
/// the `<prefix>-<suffix>` room names it already has.
pub(crate) fn room_and_provider(
    config: &WebServerConfig,
    counter: usize,
) -> (String, Option<&crate::config::Provider>) {
    if !config.production
        && let Some(room_name) = config
            .fixed_room_name
            .as_deref()
            .filter(|room_name| !room_name.is_empty())
    {
        return (
            room_name.to_string(),
            config.pool.for_room(room_name, &config.room_prefix),
        );
    }
    let provider = config.pool.select(counter);

    // The primary contributes no segment, so a single-provider deployment keeps
    // the room names it already has. Comparing exactly is sound only because
    // `is_provider_id` reserves the word case-insensitively upstream, so no
    // provider spelled `Primary` can reach this line. The asymmetry is
    // load-bearing: do not "fix" one of the two comparisons on its own.
    let segment = provider
        .map(|provider| provider.id.as_str())
        .filter(|id| *id != crate::config::PRIMARY_PROVIDER_ID)
        .map_or(String::new(), |id| format!("-{id}"));
    (
        format!("{}{segment}-{}", config.room_prefix, suffix(8)),
        provider,
    )
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
        let limit = TokenRateLimit::default();
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
        let limit = TokenRateLimit::default();
        let start = Instant::now();

        // Spend one client's budget early, then force a sweep it survives, so
        // the next sweep is a full window away.
        for _ in 0..TOKEN_RATE_LIMIT {
            assert!(limit.allow(ip(1), start + Duration::from_secs(50)));
        }
        assert!(!limit.allow(ip(1), start + Duration::from_secs(50)));
        assert!(limit.allow(ip(2), start + TOKEN_RATE_WINDOW));
        assert_eq!(
            limit.0.lock().unwrap().windows.len(),
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
        let limit = TokenRateLimit::default();
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
        assert_eq!(limit.0.lock().unwrap().windows.len(), 2);
        assert!(limit.allow(ip(3), start + TOKEN_RATE_WINDOW));
        assert_eq!(
            limit.0.lock().unwrap().windows.len(),
            1,
            "expired windows should be dropped, not accumulated"
        );
    }
}

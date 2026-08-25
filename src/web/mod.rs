use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::connect_info::IntoMakeServiceWithConnectInfo;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use serde_json::{Value, json};

use crate::accounts::{Accounts, GitHubLoginConfig};

pub use crate::accounts::{ACCOUNT_SCHEMA_VERSION, initialize_account_database};
pub use crate::current_epoch_seconds;

mod assets;
mod auth;
mod consent;
mod interviews;
mod policy;
mod recordings;
mod token;

// Flattened back into one namespace so the split is a file boundary and not an
// API change: every path that named `web::thing` before still does.
pub(crate) use {
    assets::*, auth::*, consent::*, interviews::*, policy::*, recordings::*, token::*,
};

// The whole of this module's public surface, named rather than left to fall out
// of whichever items happen to carry `pub`. A glob would re-export a future
// `pub fn` here silently; this way adding one is a deliberate line.
pub use assets::static_file;
pub use auth::login_config;
pub use token::{TOKEN_RATE_LIMIT, TokenConfig, TokenResponse, token_response};

pub const MAX_BODY_BYTES: usize = 8 * 1024;
/// A report carries a graded summary and up to sixteen feedback lines, any of
/// which can be multi-byte, so it needs far more room than a session request.
/// The browser bounds each field (`sanitizeReport` in `web/lib.js`); this is
/// the ceiling that makes those bounds sufficient whatever the text is.
pub const MAX_REPORT_BYTES: usize = 64 * 1024;
/// A webhook body is not a session request. `egress_ended` carries the room,
/// the egress request that produced it, and every file result with its own
/// location and byte counts, and a body over the limit is answered `413` and
/// retried forever rather than applied.
pub const MAX_WEBHOOK_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebServerConfig {
    pub web_dir: PathBuf,
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    pub session_secret: Option<String>,
    pub db_path: Option<PathBuf>,
    pub github_oauth_base_url: Option<String>,
    pub github_api_base_url: Option<String>,
    pub room_prefix: String,
    pub fixed_room_name: Option<String>,
    pub production: bool,
    pub compiler_explorer_enabled: bool,
    /// Number of trusted reverse proxies in front of this server. Zero, the
    /// default, keys rate limiting on the TCP peer. Anything higher reads the
    /// client from `X-Forwarded-For`, which is only safe once every request is
    /// known to pass through that many proxies you control.
    pub trusted_proxy_hops: u32,
    /// Production recording, or `None` when it is off, which is every
    /// deployment that has not provisioned the resources in
    /// `docs/recording-contract.md`.
    pub recording: Option<crate::config::RecordingConfig>,
    /// Every LiveKit project this server can hand a candidate to, primary
    /// first. The single source: the primary used to be stored here and again
    /// in three flat fields, which cost a fallback chain at every read and a
    /// dedup pass over the policy that named it twice. An empty pool means no
    /// LiveKit credentials are configured.
    pub pool: crate::config::ProviderPool,
}

/// Cloned per request by axum, so the config sits behind an `Arc`: otherwise
/// every asset fetch deep-copies the LiveKit secret and the web root path.
#[derive(Clone)]
pub(crate) struct AppState {
    config: Arc<WebServerConfig>,
    /// `None` when this server has no cookie secret and no database, and also
    /// when it has both but the database would not open. Those are different
    /// problems, so `accounts_required` separates them.
    accounts: Option<Arc<Accounts>>,
    /// Whether accounts are configured at all, which is what the browser needs
    /// to know: a server whose database is merely broken still requires a
    /// login, and must not answer as though anyone may start an interview.
    accounts_required: bool,
    /// Absent in a web-only deployment, where agents are started elsewhere.
    dispatcher: Option<Arc<dyn RoomDispatcher>>,
    /// The provider and clock the recording pipeline runs against. `None`
    /// wherever `config.recording` is `None`, which is every deployment that
    /// has not provisioned it.
    recorder: Option<crate::recording::Recorder>,
    token_limit: TokenRateLimit,

    // A separate bucket, for the same reason `/api/token` checks the session
    // before spending its own: login is reachable without any credential, so a
    // flood of it must not lock a signed-in candidate out of a token.
    login_limit: TokenRateLimit,

    /// Which provider the next room goes to. Relaxed because nothing depends on
    /// the order two concurrent requests observe, only that they observe
    /// different values.
    provider_counter: Arc<AtomicUsize>,
    /// What each project last said about its own connection-minute quota, so a
    /// project that has run out is passed over instead of handed to a candidate
    /// whose browser can only report the refusal as an unexplained socket error.
    provider_quota: ProviderQuota,
    room_authorizations: Arc<Mutex<HashMap<String, RoomAuthorization>>>,
}

/// Opens the connection the server keeps, brings its schema up to date, and
/// reclaims what the last run leaked.
///
/// Fails closed on either database step: `None` means no accounts, which every
/// handler already answers with a 503. It is never downgraded to "no accounts
/// configured here", because that would drop the sign-in gate over a bad path.
/// `initialize_accounts` runs first in both binaries and the migration retries
/// through lock contention, so reaching a failure arm means something is wrong
/// with the database itself, and saying so once at startup beats one log line
/// per request.
pub(crate) fn open_accounts(login: GitHubLoginConfig) -> Option<Arc<Accounts>> {
    let accounts = Accounts::open(login)
        .inspect_err(|error| {
            eprintln!("account database will not open, refusing account requests: {error}")
        })
        .ok()?;
    accounts
        .initialize_schema()
        .inspect_err(|error| {
            eprintln!("account database will not initialize, refusing account requests: {error}")
        })
        .ok()?;

    // Bounded to one uptime rather than never: every unverified sign-in left a
    // throwaway account row and a session row behind, and nothing reclaimed
    // either, so the database grew for the lifetime of the deployment. A
    // long-running server still wants a periodic sweep; this is the cheapest
    // correct place to put one, not the finished answer.
    //
    // Best effort, because a transient lock must not disable otherwise healthy
    // account requests. Said out loud either way, because a sweep nobody can
    // see is a database nobody notices growing.
    match crate::accounts::sweep_expired_sessions(&accounts, current_epoch_seconds() as i64) {
        Ok((0, 0)) => {}
        Ok((sessions, users)) => {
            eprintln!("swept {sessions} expired session(s) and {users} throwaway account(s)")
        }
        Err(error) => eprintln!("WARNING: session sweep failed: {error}"),
    }
    Some(Arc::new(accounts))
}

/// Not public: `/api/token` extracts `ConnectInfo`, so a bare `Router` served
/// without connect info answers every token request with a 500. Callers go
/// through [`web_service`].
pub(crate) fn web_router(
    config: WebServerConfig,
    dispatcher: Option<Arc<dyn RoomDispatcher>>,
    recorder: Option<crate::recording::Recorder>,
) -> Router {
    let login = login_config(&config);
    let accounts_required = login.is_some();

    // The binaries refuse to start in this state, because an operator can fix
    // it. This constructor is also public, and an embedder reaching it with the
    // same configuration gets a server that answers 403 to every token request
    // for a reason nothing on the wire explains. Said out loud rather than made
    // fatal: the gate itself is what the recording tests exercise, and a router
    // that panicked here could not be tested at all.
    //
    // Keyed on the OAuth pair itself rather than on `login_config`, which also
    // returns `None` for a missing session secret or database path. That is a
    // different deployment problem with a different answer on the wire, and one
    // warning describing both would be wrong about whichever it did not mean.
    let oauth_credential = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    };
    if config.recording.is_some()
        && !(oauth_credential(&config.github_client_id)
            && oauth_credential(&config.github_client_secret))
    {
        eprintln!(
            "recording is configured without a GitHub OAuth app, so every account is \
             self-declared and /api/token will refuse every interview"
        );
    }
    let accounts = login.and_then(open_accounts);
    if let (Some(accounts), Some(recorder)) = (&accounts, &recorder) {
        spawn_recording_sweeper(accounts.clone(), recorder.clone());
        spawn_delivery_worker(accounts.clone(), recorder.clone());
    }
    Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/api/token", post(token_handler))
        .route("/api/observer-token", post(observer_token_handler))
        .route("/api/login", get(login_handler).post(record_login_handler))
        .route("/api/callback", get(callback_handler))
        .route("/api/session", get(session_handler))
        .route("/api/logout", post(logout_handler))
        .route("/api/interviews", post(create_interview_handler))
        .route(
            "/api/interviews/{id}/consent",
            delete(withdraw_consent_handler),
        )
        .route(
            "/api/interviews/{id}/recording",
            post(start_recording_handler).get(recording_status_handler),
        )
        .route("/api/interviews/{id}/end", post(end_interview_handler))
        .route("/api/interviews/{id}/events", post(replay_events_handler))
        .route(
            "/api/interviews/{id}/snapshot",
            get(replay_snapshot_handler),
        )
        .route(
            crate::recording::WEBHOOK_ROUTE,
            post(recording_webhook_handler),
        )
        .route(
            crate::recording::REPLAY_ROUTE,
            get(recording_replay_handler),
        )
        .route("/api/recordings", get(list_recordings_handler))
        .route("/api/recordings/{id}", get(recording_handler))
        .route("/api/recordings/{id}/events", get(recording_events_handler))
        .route("/runtime-config.js", get(runtime_config_handler))
        .route(
            "/api/reports",
            get(list_reports_handler).post(save_report_handler),
        )
        .fallback(web_static_handler)
        .layer(axum::middleware::from_fn_with_state(
            content_security_policy_header(&config),
            security_headers,
        ))
        // Outermost, so it sees the finished response whatever produced it.
        // `web/` is 13 MB, over 11 MB of that wasm, and shipping it verbatim is
        // the one cost a candidate pays before the timer is useful to them.
        //
        // Everything except the avatar model. A .vrm is a GLB, and a GLB is a
        // container around textures that are already PNG or JPEG, so gzipping
        // jim.vrm spends 2.3 seconds of CPU per request to take 10.9 MB down to
        // 8.1 MB. Measured on loopback: 8 ms served verbatim against 2348 ms
        // served gzipped, which is most of the delay before Jim has a face. The
        // wasm keeps its compression, where the ratio is real.
        .layer({
            use tower_http::compression::predicate::{
                DefaultPredicate, NotForContentType, Predicate,
            };
            tower_http::compression::CompressionLayer::new().compress_when(
                DefaultPredicate::new().and(NotForContentType::const_new("model/gltf-binary")),
            )
        })
        .with_state(AppState {
            config: Arc::new(config),
            accounts,
            accounts_required,
            dispatcher,
            recorder,
            token_limit: TokenRateLimit::default(),
            login_limit: TokenRateLimit::default(),
            provider_counter: Arc::new(AtomicUsize::new(0)),
            provider_quota: ProviderQuota::default(),
            room_authorizations: Arc::new(Mutex::new(HashMap::new())),
        })
}

impl AppState {
    /// The response for a request that needed accounts when there are none.
    /// Not configured and configured-but-unreachable are different operator
    /// problems and must not answer alike.
    fn accounts_error(&self) -> Response {
        if self.accounts_required {
            accounts_unavailable_response()
        } else {
            account_disabled_response()
        }
    }
}

/// Rate limiting needs the peer address, which only exists when the router is
/// served with connect info attached.
/// Puts an interviewer in a room this server just named.
///
/// `/api/token` invents a room name per request in production, and the agent
/// joins exactly one room named on its command line, so nothing connected the
/// two: a production deployment handed candidates tokens for rooms no operator
/// could know the name of, and the interview never started. This is the seam
/// that closes it. It lives here because this is where rooms are named, and it
/// is a trait because `web` must not reach into the agent runtime; `main.rs`
/// supplies the implementation.
///
/// The provider is handed over rather than looked up again from the room name.
/// Re-deriving it is precisely the mistake [`room_and_provider`] documents: two
/// pools assembled by two directory scans can disagree, and the candidate then
/// holds a token for one LiveKit project while the interviewer waits in
/// another. There is one lookup, and this is its result.
///
/// Returns `false` when no interviewer will come, so the caller can refuse
/// instead of handing out a token for a room nobody will ever join.
///
/// Implementations must be idempotent per room and must not block: this is
/// called on the request path.
pub trait RoomDispatcher: Send + Sync + 'static {
    fn ensure_agent(&self, room_name: &str, provider: &crate::config::Provider) -> bool;
}

pub fn web_service(config: WebServerConfig) -> IntoMakeServiceWithConnectInfo<Router, SocketAddr> {
    web_service_with_dispatcher(config, None)
}

/// The router with a recording provider supplied rather than built.
///
/// Every failure the pipeline has to survive is the provider's: a start that
/// refuses, a stop that never arrives, a webhook that goes missing. None can be
/// produced against LiveKit on demand, so the seam is the constructor.
pub fn web_service_with_recorder(
    config: WebServerConfig,
    dispatcher: Option<Arc<dyn RoomDispatcher>>,
    recorder: crate::recording::Recorder,
) -> IntoMakeServiceWithConnectInfo<Router, SocketAddr> {
    warn_about_unfetched_vendor(&config.web_dir);
    web_router(config, dispatcher, Some(recorder))
        .into_make_service_with_connect_info::<SocketAddr>()
}

/// The dispatcher is optional because a web-only deployment is a supported
/// shape: an operator may run `codetrial web` here and `codetrial run-livekit`
/// elsewhere, and that server has no Gemini key and no business starting
/// agents. `None` preserves exactly the behaviour that mode already had.
pub fn web_service_with_dispatcher(
    config: WebServerConfig,
    dispatcher: Option<Arc<dyn RoomDispatcher>>,
) -> IntoMakeServiceWithConnectInfo<Router, SocketAddr> {
    warn_about_unfetched_vendor(&config.web_dir);

    // The whole pool, not the primary project. Rooms are routed by provider id
    // and a recording belongs to whichever project holds its room, so signing
    // every Egress call with the primary key would work for a single-provider
    // deployment and record nothing at all for the others.
    let recorder = config
        .recording
        .as_ref()
        .map(|recording| crate::recording::Recorder {
            provider: Arc::new(crate::recording::LiveKitEgress {
                pool: config.pool.clone(),
                room_prefix: config.room_prefix.clone(),
                livekit: recording.livekit.clone(),
            }),
            clock: Arc::new(crate::recording::SystemClock),
            delivery: delivery_provider(recording),
            config: recording.clone(),
        });
    web_router(config, dispatcher, recorder).into_make_service_with_connect_info::<SocketAddr>()
}

pub(crate) fn unauthorized_response() -> Response {
    json_response(
        StatusCode::UNAUTHORIZED,
        json!({ "error": "Sign in to use account reports." }),
    )
}

pub(crate) fn account_disabled_response() -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        json!({ "error": "GitHub username recording is not configured." }),
    )
}

pub(crate) fn accounts_unavailable_response() -> Response {
    json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({ "error": "The account database is unavailable." }),
    )
}

pub(crate) fn oauth_not_configured_response() -> Response {
    json_response(
        StatusCode::BAD_REQUEST,
        json!({ "error": "Submit a GitHub username to start an interview." }),
    )
}

pub(crate) fn rate_limited_response(message: &str) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::RETRY_AFTER, "60"),
        ],
        json!({ "error": message }).to_string(),
    )
        .into_response()
}

pub(crate) fn query_pairs(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            Some((query_unescape(key)?, query_unescape(value)?))
        })
        .collect()
}

pub(crate) fn query_escape(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

pub(crate) fn query_unescape(value: &str) -> Option<String> {
    let mut output = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
                output.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).ok()
}

pub(crate) fn json_response(status: StatusCode, value: Value) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "application/json"),
            // `/api/session` answers with the signed-in account name, so none
            // of these belong in a shared cache or the back/forward cache.
            (header::CACHE_CONTROL, "no-store"),
        ],
        value.to_string(),
    )
        .into_response()
}

pub(crate) fn number_json(value: f64) -> Value {
    if value.fract() == 0.0 {
        json!(value as u64)
    } else {
        json!(value)
    }
}

/// Who to charge for a token request. Behind a proxy every request arrives from
/// the same peer, so without this one bucket would cover every candidate; but
/// trusting `X-Forwarded-For` unconditionally lets any direct client forge a
/// fresh identity per request, so the header is only read when the operator has
/// declared how many proxies actually sit in front.
///
/// Each proxy appends the address it saw, so with `hops` trusted proxies the
/// client is the `hops`-th entry from the right.
pub(crate) fn client_ip(headers: &header::HeaderMap, peer: SocketAddr, hops: u32) -> IpAddr {
    if hops == 0 {
        return peer.ip();
    }
    let Some(forwarded) = headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
    else {
        return peer.ip();
    };
    let entries = forwarded
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    entries
        .len()
        .checked_sub(hops as usize)
        .and_then(|index| entries.get(index))
        .and_then(|entry| {
            // Bare address, or "ip:port" / "[v6]:port" if a proxy appended one.
            // Parsing IPv6 first keeps its colons from being read as a port.
            entry
                .parse::<IpAddr>()
                .ok()
                .or_else(|| entry.parse::<SocketAddr>().ok().map(|address| address.ip()))
        })
        .unwrap_or_else(|| peer.ip())
}

/// One body shape for both replay reads, so a caller that starts with a
/// snapshot and carries on with the tail parses one thing.
pub(crate) fn replay_body(snapshot: &crate::recording::Snapshot) -> Value {
    json!({
        "seq": snapshot.seq,
        "quotaExceeded": snapshot.quota_exceeded,
        "events": snapshot
            .events
            .iter()
            .map(|(seq, event)| json!({
                "seq": seq,
                "kind": event.kind.as_str(),
                "at": event.at,
                "payload": event.payload,
            }))
            .collect::<Vec<_>>(),
    })
}

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::connect_info::IntoMakeServiceWithConnectInfo;
use axum::extract::{ConnectInfo, OriginalUri, Path as UriPath, State};
use axum::http::{HeaderValue, Method, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{AppendHeaders, IntoResponse, Response};
use axum::routing::{delete, get, post};
use serde_json::{Value, json};

use crate::accounts::{
    Accounts, GitHubLoginConfig, GitHubOauth, GitHubProfile, MAX_INTERVIEWS_PER_USER,
    MAX_REPORTS_PER_USER, ReportSave, SESSION_TTL_SECONDS, SignedInUser, blocking,
    claim_interview_room, create_interview, create_session, delete_session, interview_for_account,
    list_reports, normalized_github_login, random_token, recorded_account_id,
    release_interview_room, save_report, session_user, valid_github_login, withdraw_consent,
};
use crate::config::{DEFAULT_DURATION_MIN, MAX_DURATION_MIN, MIN_DURATION_MIN};
use crate::token::{LivekitTokenInput, TOKEN_TTL_SECONDS, livekit_observer_token, livekit_token};

pub use crate::accounts::{ACCOUNT_SCHEMA_VERSION, initialize_account_database};
pub use crate::current_epoch_seconds;

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
const SESSION_COOKIE: &str = "codetrial_session";
const OAUTH_STATE_COOKIE: &str = "codetrial_oauth_state";
const OAUTH_STATE_TTL_SECONDS: i64 = 60 * 10;

/// `read:user` alone returns whatever address the person made public, which is
/// usually none. Delivery needs the primary verified one, and that is a
/// separate scope and a separate endpoint.
///
/// Asked for only where recording is on. A deployment that records nothing has
/// no use for a candidate's private address, and requesting it anyway would be
/// a consent screen listing an access this server never exercises.
const GITHUB_OAUTH_SCOPE: &str = "read:user";
const GITHUB_OAUTH_SCOPE_WITH_EMAIL: &str = "read:user user:email";

fn github_oauth_scope(recording: bool) -> &'static str {
    if recording {
        GITHUB_OAUTH_SCOPE_WITH_EMAIL
    } else {
        GITHUB_OAUTH_SCOPE
    }
}

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
const TOKEN_RATE_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Default)]
struct TokenRateLimit(Arc<Mutex<RateLimitState>>);

struct RateLimitState {
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
struct Window {
    started: Instant,
    count: u32,
}

/// Who to charge for a token request. Behind a proxy every request arrives from
/// the same peer, so without this one bucket would cover every candidate; but
/// trusting `X-Forwarded-For` unconditionally lets any direct client forge a
/// fresh identity per request, so the header is only read when the operator has
/// declared how many proxies actually sit in front.
///
/// Each proxy appends the address it saw, so with `hops` trusted proxies the
/// client is the `hops`-th entry from the right.
fn client_ip(headers: &header::HeaderMap, peer: SocketAddr, hops: u32) -> IpAddr {
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

impl TokenRateLimit {
    fn allow(&self, client: IpAddr, now: Instant) -> bool {
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

/// Cloned per request by axum, so the config sits behind an `Arc`: otherwise
/// every asset fetch deep-copies the LiveKit secret and the web root path.
#[derive(Clone)]
struct AppState {
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
    room_authorizations: Arc<Mutex<HashMap<String, RoomAuthorization>>>,
}

#[derive(Clone, Copy)]
struct RoomAuthorization {
    owner_id: i64,
    expires_at: u64,
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
fn open_accounts(login: GitHubLoginConfig) -> Option<Arc<Accounts>> {
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
fn web_router(
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
        .route(
            crate::recording::WEBHOOK_ROUTE,
            post(recording_webhook_handler),
        )
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
            room_authorizations: Arc::new(Mutex::new(HashMap::new())),
        })
}

/// Where the browser sends compiled-language runs. Kept in step with the
/// fallback in `web/runners.js`, because the policy below has to name the same
/// origin the page will actually reach for.
const COMPILER_EXPLORER_ORIGIN: &str = "https://godbolt.org";

/// Built once, at startup. Parsing the policy per response was both wasted
/// work and a silent fail-open: a header value that would not parse dropped
/// the whole policy from every response with nothing said about it.
/// `url_origin` rejects anything that is not header-safe, so the fallback
/// below is unreachable and exists only so a bad origin costs the LiveKit host
/// rather than the policy.
fn content_security_policy_header(config: &WebServerConfig) -> HeaderValue {
    HeaderValue::from_str(&content_security_policy(config))
        .unwrap_or_else(|_| HeaderValue::from_static("default-src 'self'"))
}

/// The honest ceiling first: `script-src` has to keep `'unsafe-eval'`, because
/// the JavaScript runner evaluates candidate code inside a blob Worker and blob
/// workers inherit this document's policy, and Pyodide compiles wasm. So this
/// is not a policy that stops script injection outright.
///
/// It still buys the things that need no eval: no framing, no `<base>`
/// rewriting, no plugins, no inline handlers (both pages carry zero), and a
/// `connect-src` that names every origin the page is allowed to talk to
/// instead of all of them.
///
/// Pyodide used to add `https://cdn.jsdelivr.net` to both `script-src` and
/// `connect-src`. It is served from `web/vendor/pyodide/` now, so a page with
/// compiled runs withdrawn names no third-party origin at all.
fn content_security_policy(config: &WebServerConfig) -> String {
    let mut connect = vec!["'self'".to_string()];
    if config.compiler_explorer_enabled {
        connect.push(COMPILER_EXPLORER_ORIGIN.to_string());
    }
    for provider in &config.pool.providers {
        connect.extend(livekit_origins(&provider.url));
    }
    if !config.production {
        // The browser checks stand up a Compiler Explorer mock and a LiveKit
        // server on loopback ports this process never learns about, so a local
        // run has to allow them. Production names its origins exactly.
        connect.extend(
            [
                "http://127.0.0.1:*",
                "http://localhost:*",
                "ws://127.0.0.1:*",
                "ws://localhost:*",
            ]
            .map(str::to_string),
        );
    }

    [
        "default-src 'self'".to_string(),
        "base-uri 'self'".to_string(),
        "object-src 'none'".to_string(),
        "frame-ancestors 'none'".to_string(),
        "form-action 'self'".to_string(),
        "style-src 'self'".to_string(),
        "font-src 'self'".to_string(),
        "img-src 'self' data: blob:".to_string(),
        "media-src 'self' blob:".to_string(),
        "worker-src 'self' blob:".to_string(),
        "script-src 'self' blob: 'unsafe-eval'".to_string(),
        // `blob:` is here for the avatar, and it is not optional. A .vrm is a
        // GLB, so its textures are always bufferView-backed, and GLTFLoader
        // mints a `blob:` URL per image and hands it to ImageBitmapLoader,
        // which reads it with `fetch`. `fetch` is governed by `connect-src`,
        // and `'self'` does not cover `blob:` any more than it does for the
        // four directives above. Without it every texture fails and the avatar
        // silently falls back to the neutral panel on Chrome.
        format!("connect-src blob: {}", connect.join(" ")),
    ]
    .join("; ")
}

/// Both schemes on the LiveKit host, because the SDK needs both: it opens the
/// signaling WebSocket at the configured `wss://` URL but also makes plain
/// HTTPS calls to the same host, region settings among them. Naming only the
/// `wss://` origin lets the socket open and then blocks those, which surfaces
/// as a connection that fails for no stated reason.
fn livekit_origins(url: &str) -> Vec<String> {
    let Some(origin) = url_origin(url) else {
        return Vec::new();
    };
    let Some((scheme, host)) = origin.split_once("://") else {
        return Vec::new();
    };
    let sibling = match scheme {
        "wss" => "https",
        "ws" => "http",
        "https" => "wss",
        "http" => "ws",
        _ => return vec![origin],
    };
    vec![format!("{sibling}://{host}"), origin]
}

/// Scheme and authority, without pulling in a URL parser for one field. The
/// printable-ASCII check is what keeps a mistyped `LIVEKIT_URL` from making the
/// whole policy unrepresentable as a header: one origin is dropped instead.
fn url_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    let printable = |value: &str| value.bytes().all(|byte| (0x21..=0x7e).contains(&byte));
    (!scheme.is_empty() && !host.is_empty() && printable(scheme) && printable(host))
        .then(|| format!("{scheme}://{host}"))
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

async fn security_headers(
    State(policy): State<HeaderValue>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(header::CONTENT_SECURITY_POLICY, policy);
    response
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

/// How often the sweeper runs. The shortest retry backoff, because a schedule
/// checked less often than its own first step is a schedule with a different
/// first step.
const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Resolves recordings nobody is looking after, now and then repeatedly.
///
/// Now, because a process that died mid-interview left rows no request will
/// ever touch again. Repeatedly, because the retry schedule is minutes long and
/// a sweep that only ran at startup would turn every provider hiccup into a
/// failed recording.
///
/// Silent when there is no runtime to spawn on. `web_router` is public and can
/// be built outside one; a router that serves without a sweeper is degraded,
/// and a panic at construction is worse.
fn spawn_recording_sweeper(accounts: Arc<Accounts>, recorder: crate::recording::Recorder) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        eprintln!("recording is configured but there is no runtime to sweep on");
        return;
    };
    handle.spawn(async move {
        loop {
            let outcome = crate::recording::sweep_recordings(&accounts, &recorder).await;
            if outcome != crate::recording::SweepOutcome::default() {
                eprintln!(
                    "recording sweep retried {} and failed {}",
                    outcome.retried, outcome.failed
                );
            }
            tokio::time::sleep(SWEEP_INTERVAL).await;
        }
    });
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
            config: recording.clone(),
        });
    web_router(config, dispatcher, recorder).into_make_service_with_connect_info::<SocketAddr>()
}

/// Says out loud at startup which vendored assets were never downloaded.
///
/// The megabytes under `web/vendor` are pinned but not committed:
/// `scripts/fetch-vendor.sh` downloads them, and `make build`, `make serve` and
/// `make web` all run it first. `cargo run -- serve` does not, and neither does
/// a deployment that copies the tree without running the fetch. The failure
/// then lands on a candidate as a Python runtime that will not start or a face
/// detector that never loads, which is a long way from the cause.
///
/// A warning rather than a refusal: an interview without the browser Python
/// runner is degraded, not broken, and a server that will not start is worse
/// than one that says what is missing.
///
/// Silent when a vendor directory has no `SHA256SUMS`, which is what makes this
/// quiet for the temporary web roots the tests stand up. The actionable state
/// is a manifest present with its files absent.
fn warn_about_unfetched_vendor(web_dir: &Path) {
    let vendor = web_dir.join("vendor");
    let Ok(entries) = std::fs::read_dir(&vendor) else {
        return;
    };

    // The vendor root carries a manifest of its own, and the file it pins,
    // `livekit-client.js`, is the one whose absence leaves the interview page
    // unable to connect at all. Walking only the subdirectories would stay
    // silent about exactly the worst case.
    let directories = std::iter::once(vendor).chain(entries.flatten().map(|entry| entry.path()));
    let mut missing = Vec::new();
    for directory in directories {
        let Ok(manifest) = std::fs::read_to_string(directory.join("SHA256SUMS")) else {
            continue;
        };
        missing.extend(
            manifest
                .lines()
                // Trimmed because a CRLF checkout would otherwise hand every
                // name a trailing carriage return and report the whole manifest
                // as missing.
                .filter_map(|line| line.split_once("  ").map(|(_, name)| name.trim()))
                .filter(|name| !name.is_empty())
                .map(|name| directory.join(name))
                .filter(|path| !path.is_file())
                .map(|path| path.display().to_string()),
        );
    }
    if !missing.is_empty() {
        eprintln!(
            "{} vendored file(s) were never fetched, so the features that need them will fail in \
             the browser; run scripts/fetch-vendor.sh:",
            missing.len()
        );
        for path in missing {
            eprintln!("  {path}");
        }
    }
}

pub fn login_config(config: &WebServerConfig) -> Option<GitHubLoginConfig> {
    let (Some(session_secret), Some(db_path)) = (
        config
            .session_secret
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        config
            .db_path
            .as_ref()
            .filter(|value| !value.as_os_str().is_empty()),
    ) else {
        return None;
    };
    let credential = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    Some(GitHubLoginConfig {
        oauth: credential(&config.github_client_id)
            .zip(credential(&config.github_client_secret))
            .map(|(client_id, client_secret)| GitHubOauth {
                client_id,
                client_secret,
            }),
        session_secret: session_secret.to_string(),
        db_path: db_path.clone(),
        oauth_base_url: config
            .github_oauth_base_url
            .clone()
            .unwrap_or_else(|| "https://github.com".to_string()),
        api_base_url: config
            .github_api_base_url
            .clone()
            .unwrap_or_else(|| "https://api.github.com".to_string()),
    })
}

async fn login_handler(State(state): State<AppState>) -> Response {
    let Some(accounts) = state.accounts.clone() else {
        return state.accounts_error();
    };
    let Some(oauth) = &accounts.config().oauth else {
        return oauth_not_configured_response();
    };
    let Ok(state_token) = random_token(24) else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not create login state." }),
        );
    };
    let cookie = set_cookie(
        OAUTH_STATE_COOKIE,
        &signed_cookie_value(&state_token, &accounts.config().session_secret),
        OAUTH_STATE_TTL_SECONDS,
        state.config.production,
    );

    // `user:email` on top of `read:user`, because `/user` returns only a public
    // address and a recording is delivered to a verified one. Escaped rather
    // than written literally: the value now contains a space.
    let location = format!(
        "{}/login/oauth/authorize?client_id={}&state={}&scope={}",
        accounts.config().oauth_base_url.trim_end_matches('/'),
        query_escape(&oauth.client_id),
        query_escape(&state_token),
        query_escape(github_oauth_scope(state.config.recording.is_some()))
    );

    (
        StatusCode::FOUND,
        [(header::LOCATION, location), (header::SET_COOKIE, cookie)],
    )
        .into_response()
}

async fn record_login_handler(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response {
    let Some(accounts) = state.accounts.clone() else {
        return state.accounts_error();
    };
    let client = client_ip(request.headers(), peer, state.config.trusted_proxy_hops);
    if !state.login_limit.allow(client, Instant::now()) {
        return rate_limited_response("Too many login attempts. Wait a minute and try again.");
    }
    let Ok(body) = to_bytes(request.into_body(), MAX_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Ok(body) = serde_json::from_slice::<Value>(&body) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "GitHub login must be JSON." }),
        );
    };
    let Some(login) = body
        .get("login")
        .and_then(Value::as_str)
        .map(normalized_github_login)
        .filter(|login| valid_github_login(login))
    else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Enter a valid GitHub username." }),
        );
    };
    let Ok(github_id) = recorded_account_id() else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not record GitHub login." }),
        );
    };
    let session_secret = accounts.config().session_secret.clone();
    let profile = GitHubProfile {
        github_id,
        login,
        avatar_url: None,

        // A typed handle proves nothing, so it can never carry a delivery
        // address. `create_session` refuses to write one for a negative id too;
        // this is the same rule stated where the value would have come from.
        verified_email: None,
    };
    let session_id = match blocking(move || create_session(&accounts, &profile)).await {
        Ok(session_id) => session_id,
        Err(error) => {
            eprintln!("could not record GitHub login: {error}");
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not record GitHub login." }),
            );
        }
    };

    // Through `json_response` so the no-store policy it documents stays in one
    // place; this route only adds the cookie on top.
    let mut response = json_response(StatusCode::OK, json!({ "ok": true }));
    let cookie = session_cookie(&session_id, &session_secret, state.config.production);
    if let Ok(cookie) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

async fn callback_handler(State(state): State<AppState>, request: Request<Body>) -> Response {
    let Some(accounts) = state.accounts.clone() else {
        return state.accounts_error();
    };
    // Without an OAuth app there is no code to exchange.
    let Some(oauth) = accounts.config().oauth.clone() else {
        return oauth_not_configured_response();
    };
    let query = query_pairs(request.uri().query().unwrap_or(""));
    let Some(code) = query.get("code").filter(|value| !value.is_empty()) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Missing GitHub callback code." }),
        );
    };
    let Some(state_token) = query.get("state").filter(|value| !value.is_empty()) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Missing GitHub callback state." }),
        );
    };
    let Some(cookie_state) = cookie_value(request.headers(), OAUTH_STATE_COOKIE)
        .and_then(|value| verified_cookie_value(&value, &accounts.config().session_secret))
    else {
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "Login state is missing or expired." }),
        );
    };
    if cookie_state != *state_token {
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "Login state did not match." }),
        );
    }

    let Ok(access_token) = github_access_token(accounts.config(), &oauth, code).await else {
        return json_response(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "GitHub token exchange failed." }),
        );
    };
    let Ok(profile) = github_profile(
        accounts.config(),
        &access_token,
        state.config.recording.is_some(),
    )
    .await
    else {
        return json_response(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "GitHub profile request failed." }),
        );
    };
    let session_secret = accounts.config().session_secret.clone();
    let Ok(session_id) = blocking(move || create_session(&accounts, &profile)).await else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not create account session." }),
        );
    };

    (
        StatusCode::FOUND,
        [(header::LOCATION, "/".to_string())],
        AppendHeaders([
            (
                header::SET_COOKIE,
                session_cookie(&session_id, &session_secret, state.config.production),
            ),
            (
                header::SET_COOKIE,
                clear_cookie(OAUTH_STATE_COOKIE, state.config.production),
            ),
        ]),
    )
        .into_response()
}

async fn session_handler(State(state): State<AppState>, request: Request<Body>) -> Response {
    // Never a constant: a server with no cookie secret and no database cannot
    // record a login at all, and telling the page to demand one anyway leaves
    // the candidate typing a username into an endpoint that answers 404.
    let login_required = state.accounts_required;
    match current_user(state.accounts.as_ref(), request.headers()).await {
        Ok(Some(user)) => json_response(
            StatusCode::OK,
            json!({
                "signedIn": true,
                "loginRequired": login_required,
                "user": {
                    "login": user.login,
                    "avatarUrl": user.avatar_url
                }
            }),
        ),
        Ok(None) => json_response(
            StatusCode::OK,
            json!({ "signedIn": false, "loginRequired": login_required }),
        ),
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not read account session." }),
        ),
    }
}

async fn logout_handler(State(state): State<AppState>, request: Request<Body>) -> Response {
    if let Some(accounts) = state.accounts.clone()
        && let Some(session_id) = cookie_value(request.headers(), SESSION_COOKIE)
            .and_then(|value| verified_cookie_value(&value, &accounts.config().session_secret))
    {
        // The cookie is cleared below either way, so a failure here does not
        // change the answer. It does leave a live session row behind until it
        // expires, which is worth a line rather than nothing.
        if let Err(error) = blocking(move || delete_session(&accounts, &session_id)).await {
            eprintln!("could not delete account session on logout: {error}");
        }
    }
    (
        StatusCode::OK,
        [(
            header::SET_COOKIE,
            clear_cookie(SESSION_COOKIE, state.config.production),
        )],
        json!({ "ok": true }).to_string(),
    )
        .into_response()
}

/// The gate both report endpoints sit behind. It is one function rather than a
/// copy in each because the two must not drift: whatever makes one of them
/// refuse a request has to make the other refuse it too, and report data is the
/// last place to discover that only one endpoint got a fix.
async fn signed_in_owner(
    state: &AppState,
    headers: &header::HeaderMap,
) -> Result<(Arc<Accounts>, SignedInUser), Response> {
    let user = match current_user(state.accounts.as_ref(), headers).await {
        Ok(Some(user)) => user,
        Ok(None) => return Err(unauthorized_response()),
        Err(_) => {
            return Err(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read account session." }),
            ));
        }
    };
    match state.accounts.clone() {
        Some(accounts) => Ok((accounts, user)),
        None => Err(state.accounts_error()),
    }
}

async fn list_reports_handler(State(state): State<AppState>, request: Request<Body>) -> Response {
    let (accounts, user) = match signed_in_owner(&state, request.headers()).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    match blocking(move || list_reports(&accounts, user.id)).await {
        Ok(reports) => json_response(StatusCode::OK, json!({ "reports": reports })),
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not read reports." }),
        ),
    }
}

async fn save_report_handler(State(state): State<AppState>, request: Request<Body>) -> Response {
    let (accounts, user) = match signed_in_owner(&state, request.headers()).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Ok(body) = to_bytes(request.into_body(), MAX_REPORT_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Report must be JSON." }),
        );
    };
    let Some(problem_id) = payload
        .get("problemId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Report is missing problemId." }),
        );
    };
    let report_id = payload
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| random_token(18).ok());
    let Some(report_id) = report_id else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not create report id." }),
        );
    };
    let problem_id = problem_id.to_string();
    let saved = {
        let report_id = report_id.clone();
        blocking(move || save_report(&accounts, user.id, &report_id, &problem_id, &payload)).await
    };
    match saved {
        Ok(ReportSave::Saved) => json_response(StatusCode::OK, json!({ "id": report_id })),
        Ok(ReportSave::NotOwner) => json_response(
            StatusCode::CONFLICT,
            json!({ "error": "Report id belongs to another user." }),
        ),

        // Not 429: waiting does not help, and telling a client to retry a
        // request that can never succeed is worse than saying it is full.
        Ok(ReportSave::AtCapacity) => json_response(
            StatusCode::INSUFFICIENT_STORAGE,
            json!({ "error": format!("This account is at its limit of {MAX_REPORTS_PER_USER} saved reports.") }),
        ),
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not save report." }),
        ),
    }
}

fn unauthorized_response() -> Response {
    json_response(
        StatusCode::UNAUTHORIZED,
        json!({ "error": "Sign in to use account reports." }),
    )
}

fn account_disabled_response() -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        json!({ "error": "GitHub username recording is not configured." }),
    )
}

fn accounts_unavailable_response() -> Response {
    json_response(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({ "error": "The account database is unavailable." }),
    )
}

fn oauth_not_configured_response() -> Response {
    json_response(
        StatusCode::BAD_REQUEST,
        json!({ "error": "Submit a GitHub username to start an interview." }),
    )
}

fn rate_limited_response(message: &str) -> Response {
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

/// Takes the credentials rather than the whole config, so posting empty ones
/// to GitHub is not something a caller can express.
async fn github_access_token(
    config: &GitHubLoginConfig,
    oauth: &GitHubOauth,
    code: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let response = crate::http_client()
        .post(format!(
            "{}/login/oauth/access_token",
            config.oauth_base_url.trim_end_matches('/')
        ))
        .header(header::ACCEPT, "application/json")
        .json(&json!({
            "client_id": oauth.client_id,
            "client_secret": oauth.client_secret,
            "code": code,
        }))
        .send()
        .await?;
    let body = response.json::<Value>().await?;
    body.get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "missing GitHub access token".into())
}

async fn github_profile(
    config: &GitHubLoginConfig,
    access_token: &str,
    recording: bool,
) -> Result<GitHubProfile, Box<dyn std::error::Error + Send + Sync>> {
    let body = crate::http_client()
        .get(format!(
            "{}/user",
            config.api_base_url.trim_end_matches('/')
        ))
        .bearer_auth(access_token)
        .header(header::USER_AGENT, "codetrial")
        .send()
        .await?
        .json::<Value>()
        .await?;
    let github_id = body
        .get("id")
        .and_then(Value::as_i64)
        .ok_or("missing GitHub id")?;
    let login = body
        .get("login")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("missing GitHub login")?;
    Ok(GitHubProfile {
        github_id,
        login: login.to_string(),
        avatar_url: body
            .get("avatar_url")
            .and_then(Value::as_str)
            .map(str::to_string),

        // Not fetched at all where recording is off, so a deployment that
        // records nothing never holds a candidate's private address and never
        // fails a sign-in because GitHub rate limited an endpoint it had no
        // reason to call.
        //
        // `None` here also clears an address stored while recording was on,
        // because `create_session` writes what it is given. That is deliberate:
        // a deployment with recording off has no basis to keep a private
        // address, and the cost of turning recording back on is one extra
        // sign-in for anyone who signed in during the gap.
        verified_email: match recording {
            true => primary_verified_email(config, access_token).await?,
            false => None,
        },
    })
}

/// The one address GitHub says is both primary and verified.
///
/// `Ok(None)` means GitHub answered and this account has no such address.
/// `Err` means GitHub did not answer, and the two must not be confused: the
/// caller writes this value into `users.email`, so folding a rate limit or a
/// 502 into "no verified address" would take the delivery address away from
/// somebody who already had one, permanently, on a transient failure of
/// somebody else's server.
///
/// Only the selected address is returned. The response lists every address
/// someone has registered, which is more about them than this pipeline has any
/// business holding.
async fn primary_verified_email(
    config: &GitHubLoginConfig,
    access_token: &str,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let body = crate::http_client()
        .get(format!(
            "{}/user/emails",
            config.api_base_url.trim_end_matches('/')
        ))
        .bearer_auth(access_token)
        .header(header::USER_AGENT, "codetrial")
        .send()
        .await?
        // A 403 from a rate limit still carries a JSON body, and that body
        // parses as a `Value` and is not an array. Without this the failure
        // reaches the `as_array` below and comes back as "no address".
        .error_for_status()?
        .json::<Value>()
        .await?;
    let addresses = body
        .as_array()
        .ok_or("GitHub /user/emails was not a list")?;
    Ok(addresses
        .iter()
        .find(|entry| {
            entry.get("primary").and_then(Value::as_bool) == Some(true)
                && entry.get("verified").and_then(Value::as_bool) == Some(true)
        })
        .and_then(|entry| entry.get("email"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .map(str::to_string))
}

async fn current_user(
    accounts: Option<&Arc<Accounts>>,
    headers: &header::HeaderMap,
) -> rusqlite::Result<Option<SignedInUser>> {
    let Some(accounts) = accounts.cloned() else {
        return Ok(None);
    };
    let Some(session_id) = cookie_value(headers, SESSION_COOKIE)
        .and_then(|value| verified_cookie_value(&value, &accounts.config().session_secret))
    else {
        return Ok(None);
    };
    blocking(move || session_user(&accounts, &session_id)).await
}

fn signed_cookie_value(value: &str, secret: &str) -> String {
    format!("{value}.{}", crate::token::sign_hs256(secret, value))
}

fn verified_cookie_value(value: &str, secret: &str) -> Option<String> {
    let (payload, signature) = value.rsplit_once('.')?;
    crate::token::verify_hs256(secret, payload, signature).then(|| payload.to_string())
}

fn cookie_value(headers: &header::HeaderMap, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix(&prefix).map(str::to_string))
}

fn set_cookie(name: &str, value: &str, max_age: i64, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{name}={value}; Max-Age={max_age}; Path=/; HttpOnly; SameSite=Lax{secure}")
}

fn clear_cookie(name: &str, secure: bool) -> String {
    set_cookie(name, "", 0, secure)
}

/// Both login paths end the same way, and the signing step is the part that
/// must not be reinvented per handler.
fn session_cookie(session_id: &str, secret: &str, secure: bool) -> String {
    set_cookie(
        SESSION_COOKIE,
        &signed_cookie_value(session_id, secret),
        SESSION_TTL_SECONDS,
        secure,
    )
}

fn query_pairs(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            Some((query_unescape(key)?, query_unescape(value)?))
        })
        .collect()
}

fn query_escape(value: &str) -> String {
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

fn query_unescape(value: &str) -> Option<String> {
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
fn generated_candidate_identity() -> String {
    format!("candidate-{}", suffix(CANDIDATE_SUFFIX_LEN))
}

const CANDIDATE_SUFFIX_LEN: usize = 6;

pub async fn static_file(root: &Path, path: &str) -> Option<PathBuf> {
    static_file_meta(root, path).await.map(|(path, _)| path)
}

/// The resolved path together with the `stat` that resolved it. Handed back
/// rather than thrown away because `static_response` needs the same metadata
/// for
/// the ETag and the content length, and asking the kernel twice for an answer
/// this walk already has is one syscall per request for nothing.
async fn static_file_meta(root: &Path, path: &str) -> Option<(PathBuf, std::fs::Metadata)> {
    let clean = path.trim_start_matches('/');
    if clean
        .split('/')
        .any(|part| part == ".." || part.starts_with('.'))
    {
        return None;
    }
    if !clean.is_empty() && Path::new(clean).extension().is_some() {
        let path = root.join(clean);
        return Some((path.clone(), file_metadata(&path).await?));
    }
    let candidates = if clean.is_empty() {
        vec![root.join("index.html")]
    } else {
        vec![
            root.join(clean),
            root.join(clean).join("index.html"),
            root.join(format!("{clean}.html")),
            root.join("index.html"),
        ]
    };

    // Sequential rather than joined: the first candidate answers almost every
    // request, and probing four paths in parallel to save a hit that rarely
    // happens costs three wasted stats on the one that usually does.
    for path in candidates {
        if let Some(metadata) = file_metadata(&path).await {
            return Some((path, metadata));
        }
    }
    None
}

/// `Path::is_file` is a blocking `stat`, and a route with no extension probes
/// up to four candidates. Four blocking syscalls on a runtime worker is four
/// too many when the async form is already used ten lines further down.
async fn file_metadata(path: &Path) -> Option<std::fs::Metadata> {
    tokio::fs::metadata(path)
        .await
        .ok()
        .filter(std::fs::Metadata::is_file)
}

fn token_duration_min(value: Option<&Value>) -> Value {
    let duration = value.and_then(crate::agent::json_number).unwrap_or(0.0);
    if duration == 0.0 || duration.is_nan() {
        json!(DEFAULT_DURATION_MIN)
    } else {
        // Fractional minutes stay fractional: the metadata is echoed back to
        // the frontend, which renders whatever the lobby asked for.
        number_json(duration.clamp(MIN_DURATION_MIN.into(), MAX_DURATION_MIN.into()))
    }
}

async fn token_handler(
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
    let (room_name, selected_provider) = room_and_provider(
        &state.config,
        state.provider_counter.fetch_add(1, Ordering::Relaxed),
    );
    // One record, so the three values cannot come from different providers.
    let Some(provider) = selected_provider else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "error": "Server is missing LiveKit credentials. Create config/codetrial.env.local or set LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET."
            }),
        );
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

async fn observer_token_handler(State(state): State<AppState>, request: Request<Body>) -> Response {
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

async fn web_static_handler(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: header::HeaderMap,
) -> Response {
    static_response(
        &state.config.web_dir,
        uri.path(),
        headers.get(header::IF_NONE_MATCH),
        method == Method::HEAD,
    )
    .await
}

async fn runtime_config_handler(State(state): State<AppState>) -> Response {
    // The origin is emitted even when runs are enabled, so the page fetches
    // exactly what the Content-Security-Policy permits. A literal in the
    // browser could drift from the policy, and the failure would be a blocked
    // request rather than anything that names the cause.
    let mut body = if state.config.compiler_explorer_enabled {
        format!(
            "globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = true;\nglobalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = \"{COMPILER_EXPLORER_ORIGIN}\";\n"
        )
    } else {
        "globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = false;\nglobalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = \"\";\n".to_string()
    };

    // Whether to show the consent step at all, and which wording it agreed to.
    // The version travels this way rather than being written into the page,
    // because the server is the side that validates it and two spellings of a
    // version are one deploy away from disagreeing.
    body.push_str(&format!(
        "globalThis.CODETRIAL_RECORDING_ENABLED = {};\nglobalThis.CODETRIAL_CONSENT_VERSION = \"{}\";\n",
        state.config.recording.is_some(),
        crate::recording::CONSENT_VERSION
    ));
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

/// `no-store` on the whole asset tree was a correct instinct about
/// `/api/session` applied one level too wide: it forbids caching 12 MB of
/// pinned wasm, so a reload mid-interview re-downloads all of it. `no-cache`
/// still revalidates on every request, and the validator below turns that
/// revalidation into a 304 rather than a resend.
const STATIC_CACHE_CONTROL: HeaderValue = HeaderValue::from_static("no-cache");

/// Vendored MediaPipe is 11.8 MB the candidate pays for before the timer is
/// useful to them, and a day of it skips the revalidation round trips. Not
/// `immutable` for a year: `web/vendor/face-detection/README.md` tells
/// maintainers to overwrite these files in place, under the same names, so a
/// year-long promise is one the upgrade instructions cannot keep.
const VENDOR_CACHE_CONTROL: HeaderValue = HeaderValue::from_static("public, max-age=86400");

/// Modification time and length together change on any edit that matters, and
/// neither costs the file read or a digest of 5.5 MB of wasm per request. Weak
/// because it is a heuristic about the file rather than a hash of its bytes.
fn file_etag(metadata: &std::fs::Metadata) -> Option<HeaderValue> {
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    HeaderValue::from_str(&format!("W/\"{}-{}\"", modified.as_nanos(), metadata.len())).ok()
}

async fn static_response(
    root: &Path,
    path: &str,
    if_none_match: Option<&HeaderValue>,
    head_only: bool,
) -> Response {
    let Some((path, metadata)) = static_file_meta(root, path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // Anchored at the web root rather than matched anywhere in the string, so
    // an operator-supplied `CODETRIAL_WEB_DIR` that happens to contain the word
    // cannot cache the whole tree. Decided before the 304 branch because a 304
    // replaces the stored headers: answering a revalidation with `no-cache`
    // would downgrade the copy the browser already holds and undo the day.
    let cache_control = if path
        .strip_prefix(root)
        .is_ok_and(|relative| relative.starts_with("vendor"))
    {
        VENDOR_CACHE_CONTROL
    } else {
        STATIC_CACHE_CONTROL
    };
    let etag = file_etag(&metadata);

    if let Some(etag) = &etag
        && Some(etag) == if_none_match
    {
        // A 304 has to repeat the validator and the `Vary` the 200 would have
        // sent. Without the validator a cache cannot tell which version it just
        // confirmed, and without `Vary` a shared cache forgets that the stored
        // body is one specific encoding and can hand gzip to a client that
        // never asked for it.
        //
        // Ahead of the HEAD branch, not after it: a conditional HEAD is a
        // revalidation like any other, and answering it 200 tells the browser
        // its cached copy was replaced when nothing changed.
        return (
            StatusCode::NOT_MODIFIED,
            [
                (header::CACHE_CONTROL, cache_control),
                (header::ETAG, etag.clone()),
                (header::VARY, HeaderValue::from_static("accept-encoding")),
            ],
        )
            .into_response();
    }

    // Answered from metadata, never from the body. Axum routes HEAD to the same
    // handler, so the avatar's "is a model published" probe would otherwise
    // read a 15 MB file into memory once per session and throw it away.
    if head_only {
        let mut response = StatusCode::OK.into_response();
        let response_headers = response.headers_mut();
        if let Some(content_type) = content_type(&path) {
            response_headers.insert(header::CONTENT_TYPE, content_type);
        }
        response_headers.insert(header::CACHE_CONTROL, cache_control);
        if let Ok(length) = HeaderValue::from_str(&metadata.len().to_string()) {
            response_headers.insert(header::CONTENT_LENGTH, length);
        }
        if let Some(etag) = etag {
            response_headers.insert(header::ETAG, etag);
        }
        return response;
    }

    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mut response = Body::from(bytes).into_response();
            let headers = response.headers_mut();
            if let Some(content_type) = content_type(&path) {
                headers.insert(header::CONTENT_TYPE, content_type);
            }
            headers.insert(header::CACHE_CONTROL, cache_control);
            if let Some(etag) = etag {
                headers.insert(header::ETAG, etag);
            }
            response
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn json_response(status: StatusCode, value: Value) -> Response {
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

/// Picks the provider first and writes its id into the room name, because the
/// agent process is launched separately and the room name is the only thing it
/// is handed. Recomputing the choice on the other side, from a pool assembled
/// by a second directory scan, is how a candidate ends up holding a token for
/// one LiveKit project while the agent waits in another.
///
/// The primary contributes no segment, so a single-provider deployment keeps
/// the `<prefix>-<suffix>` room names it already has.
fn room_and_provider(
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
    // the room names it already has.
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
/// the same nanosecond tick produced the same suffix, and `getrandom` was
/// already a dependency with `random_token` already written.
///
/// Falls back to the clock only if the OS entropy source fails, which is the
/// same behaviour as before rather than a panic in the middle of a request.
fn suffix(len: usize) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut bytes = vec![0u8; len];
    if getrandom::fill(&mut bytes).is_err() {
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

fn content_type(path: &Path) -> Option<HeaderValue> {
    let value = match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",

        // `WebAssembly.instantiateStreaming` refuses anything that is not
        // `application/wasm`, and a body with no type at all is a coin toss for
        // any proxy in between.
        Some("wasm") => "application/wasm",

        // Same argument as `wasm`: a 15 MB untyped binary is exactly what a
        // proxy in between will try to sniff. GLTFLoader reads it as an
        // arraybuffer and does not care, but nothing else in the path is asked.
        Some("vrm" | "glb") => "model/gltf-binary",
        Some("tflite" | "binarypb") => "application/octet-stream",

        // Pyodide's stdlib. Same argument again: 2.3 MB with no type is the
        // kind of body a proxy in between decides to sniff, and `nosniff` on an
        // untyped response is a promise about nothing.
        Some("zip") => "application/zip",
        _ => return None,
    };
    Some(HeaderValue::from_static(value))
}

fn number_json(value: f64) -> Value {
    if value.fract() == 0.0 {
        json!(value as u64)
    } else {
        json!(value)
    }
}

/// Records consent, before anything can be recorded.
///
/// This is the ordering the whole feature rests on: the row exists first, and
/// `/api/token` refuses without it, so there is no path where an Egress call
/// precedes a candidate agreeing to one. The refusal is the enforcement; a
/// comment saying "call this first" is not.
async fn create_interview_handler(
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    let (accounts, user) = match signed_in_owner(&state, request.headers()).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };

    // A server that records nothing has no consent to take. Refusing keeps a
    // stale page from writing rows that mean nothing.
    if state.config.recording.is_none() {
        return json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_enabled", "error": "This server does not record interviews." }),
        );
    }
    let Ok(body) = to_bytes(request.into_body(), MAX_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let version = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|body| body.get("consentVersion")?.as_str().map(str::to_string));

    // The version has to match, not merely be present. It names the wording the
    // candidate was actually shown, and a page left open across a deploy that
    // changed that wording would otherwise be recorded as having agreed to text
    // it never displayed.
    if version.as_deref() != Some(crate::recording::CONSENT_VERSION) {
        return json_response(
            StatusCode::CONFLICT,
            json!({
                "code": "consent_version_mismatch",
                "error": "The recording disclosure changed. Reload the page and read it again."
            }),
        );
    }

    let Ok(interview_id) = random_token(16) else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not start an interview." }),
        );
    };
    let now = current_epoch_seconds() as i64;
    let recorded = {
        let interview_id = interview_id.clone();
        blocking(move || {
            create_interview(
                &accounts,
                &interview_id,
                user.id,
                crate::recording::CONSENT_VERSION,
                now,
            )
        })
        .await
    };
    match recorded {
        Ok(Some(interview)) => json_response(
            StatusCode::CREATED,
            json!({
                "interviewId": interview.id,
                "consentVersion": interview.consent_version,
                "consentAt": interview.consent_at
            }),
        ),

        // Not 429, for the same reason `/api/reports` is not: waiting does not
        // help, and telling a client to retry a request that can never succeed
        // is worse than saying it is full.
        Ok(None) => json_response(
            StatusCode::INSUFFICIENT_STORAGE,
            json!({
                "code": "interview_limit_reached",
                "error": format!("This account is at its limit of {MAX_INTERVIEWS_PER_USER} interviews.")
            }),
        ),
        Err(error) => {
            eprintln!("could not record interview consent: {error}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not record consent." }),
            )
        }
    }
}

/// Takes consent back.
///
/// 204 and no body, because there is nothing to say: the interview is the
/// candidate's, the withdrawal is recorded, and what happens to an active
/// recording is the recording lifecycle's problem. This route owns the state it
/// requests, not the transition.
async fn withdraw_consent_handler(
    State(state): State<AppState>,
    UriPath(interview_id): UriPath<String>,
    request: Request<Body>,
) -> Response {
    let (accounts, user) = match signed_in_owner(&state, request.headers()).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let now = current_epoch_seconds() as i64;
    let withdrawn = {
        let accounts = accounts.clone();
        let interview_id = interview_id.clone();
        blocking(move || withdraw_consent(&accounts, &interview_id, user.id, now)).await
    };
    match withdrawn {
        // Not found and somebody else's are one answer. Telling them apart is a
        // way to enumerate other people's interview ids.
        Ok(false) => json_response(
            StatusCode::NOT_FOUND,
            json!({ "error": "No such interview." }),
        ),
        Ok(true) => {
            // The row is written first and the provider is told second, so a
            // failed stop leaves a withdrawal that is recorded and a recording
            // the sweeper will find. The other order loses the withdrawal.
            if let Some(recorder) = state.recorder.clone() {
                let found = {
                    let accounts = accounts.clone();
                    blocking(move || {
                        crate::recording::recording_for_interview(&accounts, &interview_id, user.id)
                    })
                    .await
                };
                if let Ok(Some(recording)) = found
                    && recording.state.is_active()
                    && let Err(error) = crate::recording::stop_recording(
                        &accounts,
                        &recorder,
                        &recording,
                        crate::recording::RecordingState::Failed,
                        Some("consent_withdrawn"),
                    )
                    .await
                {
                    // Still 204. The candidate said no and that is recorded;
                    // whether the provider heard it is the sweeper's problem,
                    // not theirs. Through `audit`, because the message is the
                    // provider's and a newline in it would forge a log line.
                    crate::recording::report_stop_failure(
                        &recording.id,
                        Some("consent_withdrawn"),
                        &error,
                    );
                }
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => {
            eprintln!("could not withdraw interview consent: {error}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not withdraw consent." }),
            )
        }
    }
}

fn recording_requires_consent() -> Response {
    json_response(
        StatusCode::FORBIDDEN,
        json!({
            "code": "recording_requires_consent",
            "error": "This interview is recorded. Agree to the recording notice before starting."
        }),
    )
}

/// The consent a recorded room may start under, or the response refusing it.
///
/// Returns `Ok(None)` where recording is off, which is the state every
/// deployment is in until an operator provisions it: there is no consent to
/// check because there is nothing to consent to.
/// The interview id a recorded room may start under, checked but not yet spent.
///
/// Split from the claim below on purpose. This runs before the token is signed
/// and before an interviewer is dispatched, so a request that is going to be
/// refused is refused cheaply; the claim runs after both, so a failure between
/// them does not burn a candidate's only consent and leave them unable to
/// retry.
///
/// `Ok(None)` where recording is off, which is the state every deployment is in
/// until an operator provisions it: there is no consent to check because there
/// is nothing to consent to.
async fn checked_consent(
    state: &AppState,
    accounts: &Arc<Accounts>,
    user: &SignedInUser,
    body: &[u8],
) -> Result<Option<String>, Response> {
    if state.config.recording.is_none() {
        return Ok(None);
    }
    let Some(interview_id) = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|body| body.get("interviewId")?.as_str().map(str::to_string))
        .filter(|id| !id.is_empty())
    else {
        return Err(recording_requires_consent());
    };
    let owner = user.id;
    let lookup = {
        let accounts = accounts.clone();
        let interview_id = interview_id.clone();
        blocking(move || interview_for_account(&accounts, &interview_id, owner)).await
    };
    match lookup {
        Ok(Some(interview))
            if interview.consent_withdrawn_at.is_none() && interview.room_name.is_none() =>
        {
            Ok(Some(interview.id))
        }
        Ok(_) => Err(recording_requires_consent()),
        Err(error) => {
            eprintln!("could not read interview consent: {error}");
            Err(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read interview consent." }),
            ))
        }
    }
}

/// Spends the consent on one room, or refuses.
///
/// One statement, and it answers every refusal that matters here: gone,
/// somebody else's, withdrawn, or already used. The read above is a
/// convenience; this is the decision, because two token requests racing on one
/// interview must not both win and a read followed by a write is exactly the
/// gap where they would.
///
/// The ceiling, because it is not obvious: this closes the window between
/// checking consent and minting a token, and it does not close the one between
/// minting a token and starting Egress. A candidate who withdraws in that
/// second window has a room they may join and a recording that must not begin,
/// which is the recording lifecycle's problem: the authoritative check runs
/// immediately before the provider call.
async fn claim_consent(
    accounts: &Arc<Accounts>,
    user_id: i64,
    interview_id: String,
    room_name: &str,
) -> Result<(), Response> {
    let accounts = accounts.clone();
    let room_name = room_name.to_string();
    match blocking(move || claim_interview_room(&accounts, &interview_id, user_id, &room_name))
        .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(recording_requires_consent()),
        Err(error) => {
            eprintln!("could not claim interview consent: {error}");
            Err(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read interview consent." }),
            ))
        }
    }
}

/// Starts recording the room this interview claimed.
///
/// `202`, not `200`: the provider has been asked, the row says `starting`, and
/// whether an Egress job exists is something a webhook will say. Answering
/// `200` would promise a recording that may not have begun.
async fn start_recording_handler(
    State(state): State<AppState>,
    UriPath(interview_id): UriPath<String>,
    request: Request<Body>,
) -> Response {
    let (accounts, user) = match signed_in_owner(&state, request.headers()).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some(recorder) = state.recorder.clone() else {
        return json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_enabled", "error": "This server does not record interviews." }),
        );
    };

    // The address is read from the account rather than from the request, and
    // copied into the row: the file is delivered to whoever GitHub vouched for
    // at this moment, and the account's address can change afterwards.
    let Some(recipient) = user.verified_email.clone() else {
        return json_response(
            StatusCode::FORBIDDEN,
            json!({
                "code": "recording_requires_verified_identity",
                "error": "This interview is recorded, so it needs a GitHub sign-in."
            }),
        );
    };
    let Ok(recording_id) = random_token(16) else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not start recording." }),
        );
    };

    let begun = {
        let accounts = accounts.clone();
        let recorder = recorder.clone();
        let interview_id = interview_id.clone();
        let recording_id = recording_id.clone();
        blocking(move || {
            Ok(crate::recording::begin_recording(
                &accounts,
                recorder.clock.as_ref(),
                &interview_id,
                user.id,
                &recording_id,
                &recipient,
                recorder.config.kill_switch,
            ))
        })
        .await
    };
    let (recording, mine) = match begun {
        Ok(Ok(begun)) => begun,
        Ok(Err(refusal)) => return start_refusal_response(refusal),
        Err(_) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not start recording." }),
            );
        }
    };

    // Only the caller that inserted the row calls the provider. Two concurrent
    // starts both see a `starting` row with no egress id, and if both treated
    // that as work to do there would be two Egress jobs; a row whose creator
    // never got this far is the sweeper's.
    if !mine {
        return recording_state_now(&accounts, &recording).await;
    }

    let request = crate::recording::StartEgress {
        room_name: recording.room_name.clone().unwrap_or_default(),
        template_base_url: recorder.config.template_base_url.clone(),
        bucket: recorder.config.gcs_bucket.clone(),
        filepath: crate::recording::gcs_object_path(&recorder.config.gcs_prefix, &recording.id),
        service_account_json: recorder.config.service_account_json.clone(),
        bitrate: recorder.config.bitrate,
    };
    match recorder.provider.start(&request).await {
        Ok(egress_id) => {
            let stored = {
                let accounts = accounts.clone();
                let clock = recorder.clock.clone();
                let recording_id = recording.id.clone();
                let egress_id = egress_id.clone();
                blocking(move || {
                    crate::recording::record_egress_id(
                        &accounts,
                        clock.as_ref(),
                        &recording_id,
                        &egress_id,
                    )
                })
                .await
            };
            if !stored.unwrap_or(false) {
                // Either the row already names another job, from a sweeper
                // retry that overtook this call, or the write failed. Either
                // way this attempt owns a job the row does not, so it stops the
                // one it just made rather than leaving two running.
                eprintln!(
                    "recording {} started a job its row does not name; stopping it",
                    recording.id
                );
                let _ = recorder
                    .provider
                    .stop(&egress_id, &request.room_name)
                    .await
                    .inspect_err(|error| {
                        eprintln!("recording {} left a job running: {error}", recording.id)
                    });
                return recording_state_now(&accounts, &recording).await;
            }

            // A stop can land while the provider is still answering: consent
            // withdrawn, the room finished, the candidate closed the tab. The
            // row keeps the id whatever state it is in, and this is where a job
            // that outlived its recording gets stopped. Shared with the
            // sweeper's retry, which has the same window.
            crate::recording::settle_new_egress(&accounts, &recorder, &recording.id, &egress_id)
                .await;

            return recording_state_now(&accounts, &recording).await;
        }
        Err(error) => {
            // Left in `starting`, not failed, and the row is not touched. The
            // contract promises one minute, then five, then fifteen; a row
            // moved straight to `failed` is a row that schedule never sees, and
            // counting this attempt as a retry would push the first retry out
            // to five minutes.
            eprintln!("recording {} could not start: {error}", recording.id);
        }
    }
    recording_state_now(&accounts, &recording).await
}

/// The recording as the row has it now.
///
/// Re-read rather than reported from the snapshot this handler started with,
/// because the row moves underneath it: `starting` becomes `recording` when the
/// egress id lands, and a sweeper retry can have moved it first. Answering with
/// the older copy told the browser a recording had not begun when it had.
async fn recording_state_now(
    accounts: &Arc<Accounts>,
    recording: &crate::recording::Recording,
) -> Response {
    let fresh = {
        let accounts = accounts.clone();
        let recording_id = recording.id.clone();
        blocking(move || crate::recording::recording_by_id(&accounts, &recording_id)).await
    };
    match fresh {
        Ok(Some(fresh)) => recording_accepted(&fresh),

        // Not the snapshot. The whole point of this function is to answer with
        // what the row holds, and a state nobody read is exactly what the older
        // version reported by accident. The recording id comes back either way,
        // so a caller can ask again.
        Ok(None) | Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "code": "recording_state_unknown",
                "error": "The recording was started and its state could not be read.",
                "recordingId": recording.id
            }),
        ),
    }
}

fn recording_accepted(recording: &crate::recording::Recording) -> Response {
    json_response(
        StatusCode::ACCEPTED,
        json!({ "recordingId": recording.id, "state": recording.state.as_str() }),
    )
}

fn start_refusal_response(refusal: crate::recording::StartRefusal) -> Response {
    use crate::recording::StartRefusal;
    match refusal {
        StartRefusal::NoConsent => recording_requires_consent(),
        StartRefusal::NoRoom => json_response(
            StatusCode::CONFLICT,
            json!({
                "code": "recording_has_no_room",
                "error": "Join the interview before starting the recording."
            }),
        ),
        StartRefusal::KillSwitch => json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "code": "recording_disabled",
                "error": "Recording is turned off on this server right now."
            }),
        ),
        StartRefusal::AlreadyActive => json_response(
            StatusCode::CONFLICT,
            json!({
                "code": "recording_already_active",
                "error": "This account already has an interview recording."
            }),
        ),
        StartRefusal::Database => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not start recording." }),
        ),
    }
}

/// What happened to this interview's recording.
///
/// Ids and states only. The staged object, the Drive file and the permission
/// are handles to media, and a status route that returned them would be a way
/// to reach a recording without the permission that governs it.
async fn recording_status_handler(
    State(state): State<AppState>,
    UriPath(interview_id): UriPath<String>,
    request: Request<Body>,
) -> Response {
    let (accounts, user) = match signed_in_owner(&state, request.headers()).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };

    // A server that records nothing answers exactly as one with no such
    // recording does. Three states, one reply: telling them apart is a way to
    // learn about other people's interviews and about this deployment.
    let missing = || {
        json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_found", "error": "No recording for that interview." }),
        )
    };
    if state.recorder.is_none() {
        return missing();
    }
    let found = blocking(move || {
        crate::recording::recording_for_interview(&accounts, &interview_id, user.id)
    })
    .await;
    match found {
        Ok(None) => missing(),
        Ok(Some(recording)) => {
            // Through `Failure::parse`, so only an enumerated code can reach a
            // client. The column is written by this crate today, and a status
            // route that echoed whatever was in it is one refactor away from
            // handing back a provider's error body.
            let failure = recording
                .error
                .as_deref()
                .and_then(crate::recording::Failure::parse);
            json_response(
                StatusCode::OK,
                json!({
                    "recordingId": recording.id,
                    "state": recording.state.as_str(),
                    "error": failure.map(crate::recording::Failure::as_str),

                    // What a person does about it. "failed" is not an
                    // instruction, and a candidate whose file exists and could
                    // not be delivered must not be told to record again.
                    "recovery": failure.map(crate::recording::Failure::recovery),
                    "retries": recording.retries,
                }),
            )
        }
        Err(error) => {
            eprintln!("could not read a recording status: {error}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read the recording status." }),
            )
        }
    }
}

/// The interview is over.
///
/// One of the two ways a recording stops normally, and neither is optional: the
/// browser can be closed and the `room_finished` webhook can be lost, so the
/// pipeline has to survive either arriving alone and both arriving in any
/// order.
async fn end_interview_handler(
    State(state): State<AppState>,
    UriPath(interview_id): UriPath<String>,
    request: Request<Body>,
) -> Response {
    let (accounts, user) = match signed_in_owner(&state, request.headers()).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some(recorder) = state.recorder.clone() else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let found = {
        let accounts = accounts.clone();
        let interview_id = interview_id.clone();
        blocking(move || {
            crate::recording::recording_for_interview(&accounts, &interview_id, user.id)
        })
        .await
    };
    match found {
        // Nothing recorded, so there is nothing to stop. Not a 404: the
        // interview ending is the candidate's news either way.
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Ok(Some(recording)) => {
            finish_recording(&state, &accounts, &recorder, &recording, None).await
        }
        Err(error) => {
            eprintln!("could not read a recording while ending an interview: {error}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not end the interview." }),
            )
        }
    }
}

/// Asks the provider to stop and moves the row to `finalizing`.
///
/// Idempotent by way of the transition table: `finalizing` may become itself,
/// so the second of the two normal-end paths to arrive changes nothing.
async fn finish_recording(
    _state: &AppState,
    accounts: &Arc<Accounts>,
    recorder: &crate::recording::Recorder,
    recording: &crate::recording::Recording,
    reason: Option<&str>,
) -> Response {
    use crate::recording::RecordingState;
    if !recording.state.is_active() {
        return recording_accepted(recording);
    }
    let next = if reason.is_some() {
        // A withdrawal is not a normal end. The recording is not finished, it
        // is abandoned, and the file it produced is scheduled for deletion.
        RecordingState::Failed
    } else {
        RecordingState::Finalizing
    };
    match crate::recording::stop_recording(accounts, recorder, recording, next, reason).await {
        Ok((state, _)) => json_response(
            StatusCode::ACCEPTED,
            json!({ "recordingId": recording.id, "state": state.as_str() }),
        ),
        Err(error) => {
            crate::recording::report_stop_failure(&recording.id, Some("end"), &error);
            json_response(
                StatusCode::BAD_GATEWAY,
                json!({
                    "code": "recording_stop_failed",
                    "error": "The recording could not be stopped.",
                    "recordingId": recording.id
                }),
            )
        }
    }
}

/// LiveKit's webhooks.
///
/// Unauthenticated in the session sense and authenticated in every other: the
/// signature covers the body, and nothing in it is trusted until that checks
/// out. A refusal says nothing about which step failed, because telling a
/// caller "wrong secret" apart from "wrong digest" hands them a probe.
async fn recording_webhook_handler(
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    let Some(accounts) = state.accounts.clone() else {
        return state.accounts_error();
    };
    let Some(recorder) = state.recorder.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (parts, body) = request.into_parts();
    let Ok(body) = to_bytes(body, MAX_WEBHOOK_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let authorization = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    // The key names the project, and the project owns the secret. Reading it
    // from the token rather than assuming the primary project is what lets a
    // deployment with more than one LiveKit project verify a webhook from any
    // of them; the signature check below is still what decides.
    let Some((api_key, api_secret)) = webhook_credentials(&state, &recorder, authorization) else {
        eprintln!("recording webhook refused: unknown_key");
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "webhook_signature_invalid" }),
        );
    };
    if let Err(rejection) = crate::token::verify_livekit_webhook(
        &api_key,
        &api_secret,
        authorization,
        body.as_ref(),
        recorder.clock.now().max(0) as u64,
    ) {
        eprintln!("recording webhook refused: {rejection}");
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "webhook_signature_invalid" }),
        );
    }

    let Ok(event) = serde_json::from_slice::<Value>(body.as_ref()) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "webhook_body_invalid" }),
        );
    };
    let Some(event_id) = event.get("id").and_then(Value::as_str).map(str::to_string) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "webhook_body_invalid" }),
        );
    };

    // Claimed before anything is acted on. A retry is a fresh, validly signed
    // message with the same id, and acting on it twice is how one recording
    // gets stopped, transferred, or failed more than once.
    //
    // The ceiling: two identical deliveries arriving at once both find the
    // claim unapplied and both apply it. That is deliberate, because the
    // alternative is a lock held across a provider call, and every transition
    // below is idempotent by the table, so the outcome is the same one twice.
    let fresh = {
        let accounts = accounts.clone();
        let now = recorder.clock.now();
        blocking(move || crate::recording::claim_webhook_event(&accounts, &event_id, now)).await
    };
    match fresh {
        Ok(true) => {}

        // A duplicate is a success. LiveKit retries anything it did not get a
        // 2xx for, so answering anything else asks for it forever.
        Ok(false) => return StatusCode::OK.into_response(),
        Err(error) => {
            eprintln!("could not record a webhook event: {error}");
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "webhook_not_recorded" }),
            );
        }
    }

    if !apply_webhook(&state, &accounts, &recorder, &event, &api_key).await {
        // Not marked applied, so LiveKit's retry gets another attempt at it.
        // There is no compensating write here on purpose: the earlier version
        // deleted the claim, and a delete that itself failed lost the event.
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "error": "webhook_not_applied" }),
        );
    }

    let applied = {
        let accounts = accounts.clone();
        let event_id = event
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let now = recorder.clock.now();
        blocking(move || crate::recording::mark_webhook_applied(&accounts, &event_id, now)).await
    };
    if let Err(error) = applied {
        // The work is done and the ledger does not know. A redelivery would
        // repeat a transition that is idempotent by the table, so this is a
        // line rather than a failure.
        eprintln!("could not mark a webhook event applied: {error}");
    }
    StatusCode::OK.into_response()
}

/// The secret belonging to the key a webhook claims signed it.
///
/// `None` when no configured project holds that key, which is a refusal: a
/// message signed by a project this server does not serve is not this server's
/// message. Looking the key up rather than assuming the primary project is what
/// makes a multi-project deployment work at all, since a room belongs to one
/// project and the others' secrets verify nothing it signed.
///
/// There is deliberately no separate webhook secret. LiveKit signs with the
/// project's own API key and secret, so a third value would be one an operator
/// believed was checked.
fn webhook_credentials(
    state: &AppState,
    recorder: &crate::recording::Recorder,
    authorization: &str,
) -> Option<(String, String)> {
    let key = crate::token::livekit_webhook_key(authorization)?;
    if let Some(livekit) = &recorder.config.livekit
        && livekit.api_key == key
    {
        return Some((livekit.api_key.clone(), livekit.api_secret.clone()));
    }
    state
        .config
        .pool
        .providers
        .iter()
        .find(|provider| provider.api_key == key)
        .map(|provider| (provider.api_key.clone(), provider.api_secret.clone()))
}

/// Whether the project that signed a webhook is the one that owns this room.
///
/// The room name routes to a provider exactly as `/api/token` routed it, and
/// the key that signed the message has to be that provider's. The recording
/// override only substitutes a different key for the same project, so it is
/// checked against the room's provider rather than instead of it.
fn signed_by_the_rooms_project(
    state: &AppState,
    recorder: &crate::recording::Recorder,
    room_name: Option<&str>,
    signed_by: &str,
) -> bool {
    let Some(room_name) = room_name else {
        // A tombstoned recording has given its room name up, and there is
        // nothing left for a webhook to change.
        return false;
    };
    let Some(provider) = state
        .config
        .pool
        .for_room(room_name, &state.config.room_prefix)
    else {
        return false;
    };

    // Either key for that project. The override is a different key for the same
    // project, not a different project, and LiveKit signs a webhook with
    // whichever key the project's webhook configuration names, which is not
    // necessarily the one this server makes Egress calls with.
    if provider.api_key == signed_by {
        return true;
    }
    recorder.config.livekit.as_ref().is_some_and(|livekit| {
        livekit.api_key == signed_by
            && crate::config::same_livekit_project(&livekit.url, &provider.url)
    })
}

/// Whether the event was dealt with.
///
/// `false` asks the caller to give the claim back so LiveKit delivers it again.
/// The case that matters is an egress event arriving before this server has
/// written down the egress id it names: dropping it there would lose the only
/// notice that a recording started, ended, or failed.
async fn apply_webhook(
    state: &AppState,
    accounts: &Arc<Accounts>,
    recorder: &crate::recording::Recorder,
    event: &Value,
    signed_by: &str,
) -> bool {
    use crate::recording::RecordingState;
    let kind = event
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or_default();

    // lowerCamelCase, unlike the Twirp call that started this: upstream
    // marshals the webhook with protojson directly. The contract document is
    // where that asymmetry is explained.
    let egress_id = event
        .pointer("/egressInfo/egressId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let status = event
        .pointer("/egressInfo/status")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let found = match kind {
        "room_finished" => {
            let room = event
                .pointer("/room/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let accounts = accounts.clone();
            blocking(move || crate::recording::recording_for_room(&accounts, &room)).await
        }
        "egress_started" | "egress_updated" | "egress_ended" => {
            let Some(egress_id) = egress_id else {
                return true;
            };
            let accounts = accounts.clone();
            blocking(move || crate::recording::recording_for_egress(&accounts, &egress_id)).await
        }

        // Acknowledged and dropped. Answering anything else asks LiveKit to
        // keep sending events this pipeline has no opinion about.
        _ => return true,
    };
    let recording = match found {
        Ok(Some(recording)) => recording,

        // No recording for this event. For a room that is not this server's,
        // that is the end of it; for an egress event it usually means the start
        // response has not been written down yet, and this is the only notice
        // that job will ever get.
        Ok(None) => return kind == "room_finished",
        Err(error) => {
            eprintln!("could not read a recording for a webhook: {error}");
            return false;
        }
    };

    // A valid signature proves which project sent this, not which project owns
    // the room it names. Without this, any configured project could end another
    // project's recording by naming its room.
    if !signed_by_the_rooms_project(state, recorder, recording.room_name.as_deref(), signed_by) {
        eprintln!(
            "recording {} was named by a webhook from another project",
            recording.id
        );
        return true;
    }

    match kind {
        "room_finished" => finish_recording(state, accounts, recorder, &recording, None)
            .await
            .status()
            .is_success(),
        "egress_started" => {
            let accounts = accounts.clone();
            let clock = recorder.clock.clone();
            let id = recording.id.clone();
            blocking(move || {
                crate::recording::transition(
                    &accounts,
                    clock.as_ref(),
                    &id,
                    RecordingState::Recording,
                    None,
                )
            })
            .await
            .is_ok()
        }
        "egress_ended" | "egress_updated" => {
            // The provider's own words about how it ended. `EGRESS_COMPLETE` is
            // the only one that produced a file worth moving. The object this
            // recording is going to transfer, not any file the job happened to
            // write.
            let expected_object =
                crate::recording::gcs_object_path(&recorder.config.gcs_prefix, &recording.id);
            let (next, failure) = match status {
                // A completion with nothing in it is not a completion. A
                // missing file result, or one of no bytes, is how a truncated
                // recording arrives, and sending it through the transfer shared
                // a zero-byte video with a candidate.
                "EGRESS_COMPLETE"
                    if crate::recording::completed_with_output(event, &expected_object) =>
                {
                    (RecordingState::Transferring, None)
                }
                "EGRESS_COMPLETE" => (
                    RecordingState::Failed,
                    Some(crate::recording::Failure::PartialOutput),
                ),
                "EGRESS_FAILED" => (
                    RecordingState::Failed,
                    Some(crate::recording::Failure::Egress),
                ),
                "EGRESS_ABORTED" => (
                    RecordingState::Failed,
                    Some(crate::recording::Failure::Aborted),
                ),
                "EGRESS_LIMIT_REACHED" => (
                    RecordingState::Failed,
                    Some(crate::recording::Failure::LimitReached),
                ),

                // `EGRESS_ACTIVE` and `EGRESS_STARTING` on an update say
                // nothing new, and in particular do not say the job has
                // stopped: marking it stopped here would tell the orphan sweep
                // to stop watching a job that is still running.
                _ => return true,
            };

            // Terminal, whichever way it ended, so the row stops being one the
            // sweeper has to chase. A failure here fails the whole application,
            // because the alternative is a terminal row that looks stopped and
            // a job that is not.
            let stopped = {
                let accounts = accounts.clone();
                let clock = recorder.clock.clone();
                let id = recording.id.clone();
                blocking(move || crate::recording::mark_stopped(&accounts, clock.as_ref(), &id))
                    .await
            };
            if stopped.is_err() {
                return false;
            }
            let accounts = accounts.clone();
            let clock = recorder.clock.clone();
            let id = recording.id.clone();
            let reason = failure.map(|failure| failure.as_str().to_string());
            let moved = blocking(move || {
                crate::recording::transition(
                    &accounts,
                    clock.as_ref(),
                    &id,
                    next,
                    reason.as_deref(),
                )
            })
            .await;

            // After the transition, and only when it moved. A late
            // `EGRESS_FAILED` for a row that has already advanced is refused by
            // the table, and an audit line written first said a recording had
            // failed while its status said otherwise.
            if let (Some(failure), Ok(Some((_, true)))) = (failure, &moved) {
                crate::recording::audit(
                    "recording_failed",
                    &recording.id,
                    &[
                        ("reason", failure.as_str()),
                        ("recovery", failure.recovery()),
                    ],
                );
            }
            moved.is_ok()
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
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

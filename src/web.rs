use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::connect_info::IntoMakeServiceWithConnectInfo;
use axum::extract::{ConnectInfo, OriginalUri, State};
use axum::http::{HeaderValue, Method, Request, StatusCode, header};
use axum::middleware::Next;
use axum::response::{AppendHeaders, IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

use crate::accounts::{
    Accounts, GitHubLoginConfig, GitHubOauth, GitHubProfile, MAX_REPORTS_PER_USER, ReportSave,
    SESSION_TTL_SECONDS, SignedInUser, blocking, create_session, delete_session, list_reports,
    normalized_github_login, random_token, recorded_account_id, save_report, session_user,
    valid_github_login,
};
use crate::config::{DEFAULT_DURATION_MIN, MAX_DURATION_MIN, MIN_DURATION_MIN};
use crate::token::{LivekitTokenInput, livekit_token};

pub use crate::accounts::{ACCOUNT_SCHEMA_VERSION, initialize_account_database};
pub use crate::current_epoch_seconds;

pub const MAX_BODY_BYTES: usize = 8 * 1024;
/// A report carries a graded summary and up to sixteen feedback lines, any of
/// which can be multi-byte, so it needs far more room than a session request.
/// The browser bounds each field (`sanitizeReport` in `web/lib.js`); this is
/// the ceiling that makes those bounds sufficient whatever the text is.
pub const MAX_REPORT_BYTES: usize = 64 * 1024;
const SESSION_COOKIE: &str = "codetrial_session";
const OAUTH_STATE_COOKIE: &str = "codetrial_oauth_state";
const OAUTH_STATE_TTL_SECONDS: i64 = 60 * 10;

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
    token_limit: TokenRateLimit,

    // A separate bucket, for the same reason `/api/token` checks the session
    // before spending its own: login is reachable without any credential, so a
    // flood of it must not lock a signed-in candidate out of a token.
    login_limit: TokenRateLimit,

    /// Which provider the next room goes to. Relaxed because nothing depends on
    /// the order two concurrent requests observe, only that they observe
    /// different values.
    provider_counter: Arc<AtomicUsize>,
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
fn web_router(config: WebServerConfig, dispatcher: Option<Arc<dyn RoomDispatcher>>) -> Router {
    let login = login_config(&config);
    let accounts_required = login.is_some();
    let accounts = login.and_then(open_accounts);
    Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/api/token", post(token_handler))
        .route("/api/login", get(login_handler).post(record_login_handler))
        .route("/api/callback", get(callback_handler))
        .route("/api/session", get(session_handler))
        .route("/api/logout", post(logout_handler))
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
            token_limit: TokenRateLimit::default(),
            login_limit: TokenRateLimit::default(),
            provider_counter: Arc::new(AtomicUsize::new(0)),
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

/// The dispatcher is optional because a web-only deployment is a supported
/// shape: an operator may run `codetrial web` here and `codetrial run-livekit`
/// elsewhere, and that server has no Gemini key and no business starting
/// agents. `None` preserves exactly the behaviour that mode already had.
pub fn web_service_with_dispatcher(
    config: WebServerConfig,
    dispatcher: Option<Arc<dyn RoomDispatcher>>,
) -> IntoMakeServiceWithConnectInfo<Router, SocketAddr> {
    warn_about_unfetched_vendor(&config.web_dir);
    web_router(config, dispatcher).into_make_service_with_connect_info::<SocketAddr>()
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
    let location = format!(
        "{}/login/oauth/authorize?client_id={}&state={}&scope=read:user",
        accounts.config().oauth_base_url.trim_end_matches('/'),
        query_escape(&oauth.client_id),
        query_escape(&state_token)
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
    let Ok(profile) = github_profile(accounts.config(), &access_token).await else {
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
    })
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
    let identity = request
        .get("candidateIdentity")
        .and_then(Value::as_str)
        .filter(|identity| valid_candidate_identity(identity))
        .unwrap_or(default_identity);
    let metadata = json!({ "problemId": problem_id, "durationMin": duration_min }).to_string();

    Ok(TokenResponse {
        token: livekit_token(LivekitTokenInput {
            api_key: config.api_key,
            api_secret: config.api_secret,
            name: "Candidate",
            identity,
            room: room_name,
            metadata: &metadata,
            now_seconds,
            agent: false,
        })?,
        server_url: config.server_url.to_string(),
        room_name: room_name.to_string(),
    })
}

/// The identity handed out when the browser supplies none. Shares a definition
/// with [`valid_candidate_identity`] rather than repeating the shape, so the
/// server cannot issue something it would refuse to accept back.
fn generated_candidate_identity() -> String {
    format!("candidate-{}", suffix(CANDIDATE_SUFFIX_LEN))
}

const CANDIDATE_SUFFIX_LEN: usize = 6;

fn valid_candidate_identity(identity: &str) -> bool {
    let Some(suffix) = identity.strip_prefix("candidate-") else {
        return false;
    };
    suffix.len() == CANDIDATE_SUFFIX_LEN
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

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
    if state.accounts.is_none() {
        return state.accounts_error();
    }
    match current_user(state.accounts.as_ref(), request.headers()).await {
        Ok(Some(_)) => {}
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

    // Validate the request before starting anything external. Otherwise a
    // malformed body can leave an interviewer running for a request that got a
    // 400 response.
    if let Some(dispatcher) = &state.dispatcher
        && !dispatcher.ensure_agent(&room_name, provider)
    {
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

    json_response(
        StatusCode::OK,
        json!({
            "token": response.token,
            "serverUrl": response.server_url,
            "roomName": response.room_name
        }),
    )
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
    let body = if state.config.compiler_explorer_enabled {
        format!(
            "globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = true;\nglobalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = \"{COMPILER_EXPLORER_ORIGIN}\";\n"
        )
    } else {
        "globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = false;\nglobalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = \"\";\n".to_string()
    };
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

    /// Three places agree on this shape: the browser that generates and stores
    /// it, the validator that accepts it back, and the server-side fallback. If
    /// they drift, a client identity is silently refused and every reload
    /// becomes a new participant again, which is the failure the stored
    /// identity exists to prevent.
    #[test]
    fn the_generated_identity_satisfies_the_validator_that_accepts_it_back() {
        let generated = generated_candidate_identity();

        assert!(
            valid_candidate_identity(&generated),
            "the server issued an identity its own validator rejects: {generated}"
        );
        assert!(valid_candidate_identity("candidate-a1b2c3"));
        assert!(!valid_candidate_identity("interviewer-interview-local"));
        assert!(!valid_candidate_identity("candidate-a1b2c"));
        assert!(!valid_candidate_identity("candidate-a1b2c3d"));
        assert!(!valid_candidate_identity("candidate-A1B2C3"));
        assert!(!valid_candidate_identity("candidate-a1b2c!"));
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

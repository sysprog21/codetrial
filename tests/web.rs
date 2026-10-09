mod common;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use codetrial::accounts::MAX_REPORTS_PER_USER;
use codetrial::config::{
    DEFAULT_DURATION_MIN, DEFAULT_WEB_DIR, MAX_DURATION_MIN, MIN_DURATION_MIN,
};
use codetrial::livekit::candidate_identity_matches;
use codetrial::runtime::{
    TOPIC_CODE_UPDATE, TOPIC_CONTROL, TOPIC_INTEGRITY, TOPIC_REPORT, TOPIC_TEST_RESULTS,
    TOPIC_TRANSCRIPTION,
};
use codetrial::token::{
    LivekitTokenInput, TOKEN_TTL_SECONDS, livekit_observer_token, livekit_room_admin_token,
    livekit_token,
};
use codetrial::web::{
    AgentJob, DispatchRefusal, MAX_BODY_BYTES, MAX_REPORT_BYTES, READ_RATE_LIMIT,
    REPLAY_RATE_LIMIT, RoomDispatcher, TOKEN_RATE_LIMIT, TokenConfig, WebServerConfig,
    initialize_account_database, login_config, static_file_meta, token_response,
};

#[path = "common/http.rs"]
mod http;
use http::{client as http_client, client_builder as http_client_builder};

type HmacSha256 = Hmac<Sha256>;

fn claims(token: &str) -> Value {
    serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(token.split('.').nth(1).unwrap())
            .unwrap(),
    )
    .unwrap()
}

/// The resolved path alone. The resolver hands back the matched key and the
/// metadata beside it, and these tests care about neither.
async fn resolved(root: &Path, path: &str) -> Option<std::path::PathBuf> {
    static_file_meta(root, path).await.map(|(_, path, _)| path)
}

/// A path no directory occupies, so the handler has nothing on disk to find.
fn absent_web_dir(label: &str) -> std::path::PathBuf {
    unique_temp_path(label, "")
}

/// Process id and nanoseconds, so two tests in one binary and two binaries in
/// one suite cannot collide on it.
fn unique_temp_path(label: &str, suffix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "codetrial-{label}-{}-{}{suffix}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

/// Pins a file's modification time so a cache test can state the exact case it
/// means instead of hoping the clock cooperates.
fn set_modified(path: &Path, at: SystemTime) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

/// The node shapes a judge may declare. The browser decides the same thing
/// again in `candidateTypeMatches`, and
/// `every_supported_arg_type_is_one_the_browser_matches` holds the two
/// together.
const SUPPORTED_ARG_TYPES: [&str; 8] = [
    "linkedList",
    "linkedListArray",
    "randomList",
    "graphNode",
    "binaryTree",
    "nextTree",
    "treeNodeValue",
    "cyclePos",
];

/// A pool holding just the primary, which is what a single-project deployment
/// has. The credentials used to sit in three flat `WebServerConfig` fields
/// beside the pool; the pool is now the only place they live.
fn primary_pool(url: &str, api_key: &str, api_secret: &str) -> codetrial::config::ProviderPool {
    codetrial::config::ProviderPool {
        providers: vec![codetrial::config::Provider {
            id: codetrial::config::PRIMARY_PROVIDER_ID.to_string(),
            url: url.to_string(),
            api_key: api_key.to_string(),
            api_secret: api_secret.to_string(),
            google_api_keys: Vec::new(),
        }],
    }
}

fn provider(id: &str, host: &str) -> codetrial::config::Provider {
    codetrial::config::Provider {
        id: id.to_string(),
        url: format!("wss://{host}.livekit.cloud"),
        api_key: format!("{id}-key"),
        api_secret: format!("{id}-secret"),
        google_api_keys: vec![format!("{id}-google")],
    }
}

fn web_config() -> WebServerConfig {
    WebServerConfig {
        tasks: None,
        web_dir: std::env::temp_dir(),
        github_client_id: None,
        github_client_secret: None,
        session_secret: None,
        db_path: None,
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: "interview".to_string(),
        fixed_room_name: None,
        production: false,
        compiler_explorer_enabled: true,
        trusted_proxy_hops: 0,
        recording: None,
        pool: primary_pool("wss://example.livekit.cloud", "devkey", "devsecret"),
        probe_provider_quota: false,
    }
}

/// Writes the row `create_session` would have written for `token`. These
/// fixtures pin explicit user ids, so they insert directly instead of signing
/// in, but the id column still holds the digest rather than the cookie value.
fn insert_session(connection: &rusqlite::Connection, token: &str, user_id: i64) {
    connection
        .execute(
            "INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES (?1, ?2, 9999999999, 1)",
            (codetrial::accounts::session_key(token), user_id),
        )
        .unwrap();
}

fn signed_in_web_config(label: &str) -> (WebServerConfig, String, std::path::PathBuf) {
    let db_path = account_db_path(label);
    initialize_account_database(&db_path).unwrap();
    {
        let connection = rusqlite::Connection::open(&db_path).unwrap();
        connection
            .execute(
                "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (1, 101, 'one', 1, 1)",
                [],
            )
            .unwrap();
        insert_session(&connection, "session-one", 1);
    }

    let mut config = web_config();
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(db_path.clone());
    (
        config,
        format!(
            "codetrial_session={}",
            signed_cookie("session-one", "session-secret")
        ),
        db_path,
    )
}

/// A SQLite database in WAL mode is three files, and removing only the first
/// left the other two behind on every run: the system temp directory had
/// collected 2657 of them. `src/accounts.rs` has always cleaned all three for
/// its own scratch databases; this side never learned to.
///
/// The main file must exist and be removed, so a test that was asserting the
/// database existed still does. The sidecars do not: WAL leaves them behind
/// only if the
/// database was actually opened, and a test that never opened it is not broken.
async fn remove_database(path: impl AsRef<std::path::Path>) {
    let path = path.as_ref();

    // Dropping the router cancels recording workers. Their cancellation must
    // run before Windows releases the last database handle; keep yielding to
    // the runtime rather than sleeping on its thread or ignoring deletion.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        match fs::remove_file(path) {
            Ok(()) => break,
            Err(error)
                if cfg!(windows)
                    && error.raw_os_error() == Some(32)
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("remove test database {}: {error}", path.display()),
        }
    }
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

fn account_db_path(label: &str) -> std::path::PathBuf {
    unique_temp_path(label, ".db")
}

/// A server with accounts enabled and nobody signed in yet, which is where
/// every login test starts.
async fn account_server(label: &str) -> (String, TestServer, std::path::PathBuf, reqwest::Client) {
    let db_path = account_db_path(label);
    initialize_account_database(&db_path).unwrap();
    let mut config = web_config();
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(db_path.clone());
    let (base, server) = spawn_web_server(config).await;
    (base, server, db_path, http_client())
}

/// Signs in the way the browser does, so a test that cares about who owns what
/// does not have to hand-write rows and cookie signatures.
async fn record_login(client: &reqwest::Client, base: &str, handle: &str) -> String {
    let response = client
        .post(format!("{base}/api/login"))
        .json(&json!({ "login": handle }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "recording {handle} should succeed");
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|cookie| cookie.starts_with("codetrial_session="))
        .expect("login should set a session cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

/// A LiveKit token names its project in `iss` and proves it in the signature.
/// Checking only `iss` would miss a token minted with one provider's key and
/// another's secret, which the SFU rejects and the response body does not show.
fn verify_signature(token: &str, secret: &str) -> bool {
    // A JWS is `<signed>.<signature>`, the same shape `signed_cookie` builds,
    // so re-signing and comparing beats a second HMAC spelled out here.
    let (signed, _) = token.rsplit_once('.').unwrap();
    signed_cookie(signed, secret) == token
}

fn signed_cookie(value: &str, secret: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(value.as_bytes());
    format!(
        "{value}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

/// Stop accepting requests and drain connections before removing a database.
/// Aborting only the listener leaves Axum's connection tasks holding SQLite
/// files open, which Windows refuses to unlink.
struct TestServer {
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Result<(), std::io::Error>>,
}

impl TestServer {
    async fn shutdown(self) {
        let _ = self.shutdown.send(());
        tokio::time::timeout(Duration::from_secs(10), self.task)
            .await
            .expect("test server should drain its connections")
            .expect("test server should not panic")
            .expect("test server should shut down cleanly");
    }
}

fn spawn_test_server(
    listener: tokio::net::TcpListener,
    router: axum::extract::connect_info::IntoMakeServiceWithConnectInfo<
        axum::Router,
        std::net::SocketAddr,
    >,
) -> TestServer {
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    TestServer { shutdown, task }
}

async fn spawn_web_server(config: WebServerConfig) -> (String, TestServer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = spawn_test_server(listener, codetrial::web::web_service(config));
    (format!("http://{addr}"), server)
}

/// Records what it was asked to staff, so a test can assert that the room a
/// candidate is handed is the room an interviewer was sent to, on the LiveKit
/// project whose secret signed that candidate's token.
#[derive(Default)]
struct RecordingDispatcher {
    staffed: std::sync::Mutex<Vec<(String, String)>>,
    /// Set to refuse, standing in for a process already at capacity.
    at_capacity: std::sync::atomic::AtomicBool,
    /// Set to refuse, standing in for a fixed room still finishing a report.
    finalizing: std::sync::atomic::AtomicBool,
}

impl RoomDispatcher for RecordingDispatcher {
    fn ensure_agent(
        &self,
        room_name: &str,
        provider: &codetrial::config::Provider,
        job: AgentJob,
    ) -> Result<(), DispatchRefusal> {
        // `/api/token` starts interviews only; a task attempt has its own
        // route.
        assert!(
            matches!(job, AgentJob::Interview),
            "an interview start dispatched a task"
        );
        if self.at_capacity.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(DispatchRefusal::AtCapacity);
        }
        if self.finalizing.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(DispatchRefusal::Finalizing);
        }
        self.staffed
            .lock()
            .unwrap()
            .push((room_name.to_string(), provider.url.clone()));
        Ok(())
    }
}

impl RecordingDispatcher {
    fn rooms(&self) -> Vec<String> {
        self.staffed
            .lock()
            .unwrap()
            .iter()
            .map(|(room_name, _)| room_name.clone())
            .collect()
    }

    fn staffed(&self) -> Vec<(String, String)> {
        self.staffed.lock().unwrap().clone()
    }
}

async fn spawn_web_server_with_dispatcher(
    config: WebServerConfig,
    dispatcher: std::sync::Arc<RecordingDispatcher>,
) -> (String, TestServer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = spawn_test_server(
        listener,
        codetrial::web::web_service_with_dispatcher(config, Some(dispatcher)),
    );
    (format!("http://{addr}"), server)
}

/// The OAuth fixtures every caller of the two GitHub stubs configures: the
/// client id and the secret each test writes into its `WebServerConfig`, and
/// the `?code=` its callback request carries. Credentials of a server that
/// exists only inside this test binary, so nothing here is a real secret.
const MOCK_GITHUB_CLIENT_ID: &str = "client";
const MOCK_GITHUB_CLIENT_SECRET: &str = "secret";
const MOCK_GITHUB_CODE: &str = "ok";

type MockGitHubResponse = (
    axum::http::StatusCode,
    [(axum::http::HeaderName, &'static str); 1],
    String,
);

/// The token a stub has issued so far, `None` before its first successful
/// exchange. Cloned out from under the guard at every read so no lock is held
/// across an await.
type MockGitHubIssued = std::sync::Arc<std::sync::Mutex<Option<String>>>;

/// Distinguishes tokens minted within the same clock tick, which two stubs
/// spawned back to back in one test can be.
static MOCK_GITHUB_MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn mock_github_mint() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let sequence = MOCK_GITHUB_MINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("mock-github-token-{nanos:x}-{sequence}")
}

/// GitHub's answer to a request carrying a token it never issued.
fn mock_github_unauthorized() -> MockGitHubResponse {
    (
        axum::http::StatusCode::UNAUTHORIZED,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        json!({"message":"Bad credentials"}).to_string(),
    )
}

/// Whether this request may read the account: only if the stub has issued a
/// token and this is that token.
fn mock_github_authorized(headers: &axum::http::HeaderMap, issued: &MockGitHubIssued) -> bool {
    let Some(token) = issued.lock().unwrap().clone() else {
        return false;
    };
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some(format!("Bearer {token}").as_str())
}

/// GitHub as the sign-in path sees it, with the two checks that make the
/// handshake load bearing: the token endpoint answers only the fixture
/// credentials, and the API endpoints answer only the token it issued.
///
/// Both checks are here because without them the stub answered every caller
/// the same profile, so a `github_access_token` that never contacted GitHub
/// and returned a constant, or the empty string, completed sign-in exactly as
/// the real one does and no test could tell the difference.
///
/// The token is issued by the exchange rather than fixed in advance, which is
/// two separate properties and both are needed. Until a POST carrying the
/// fixture credentials succeeds this stub holds no token at all, so an
/// implementation that skipped the exchange has nothing `/user` accepts,
/// whatever string it returns. And the issued value mixes a clock reading with
/// a process-wide counter, so it cannot be spelled ahead of time either --
/// which a value derived from the stub's address could be, the code under test
/// being handed that address.
///
/// `emails` is the entire difference between the two stubs built on this, so
/// it is the only parameter: one passes an address list, the other the 403
/// GitHub sends while rate limiting. Everything else has to stay identical or
/// the rate-limit test would be comparing two servers that differ in more than
/// the failure it is about.
async fn spawn_github_stub(emails: (axum::http::StatusCode, Value)) -> (String, TestServer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let emails = std::sync::Arc::new(emails);
    let issued = MockGitHubIssued::default();
    let exchange_issued = issued.clone();
    let user_issued = issued.clone();
    let router = axum::Router::new()
        .route(
            "/login/oauth/access_token",
            axum::routing::post(move |body: String| {
                let exchange_issued = exchange_issued.clone();
                async move {
                    let posted = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
                    let field = |name: &str| {
                        posted
                            .get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    if field("client_id") != MOCK_GITHUB_CLIENT_ID
                        || field("client_secret") != MOCK_GITHUB_CLIENT_SECRET
                        || field("code") != MOCK_GITHUB_CODE
                    {
                        return (
                            axum::http::StatusCode::BAD_REQUEST,
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            json!({"error":"bad_verification_code"}).to_string(),
                        );
                    }
                    let token = mock_github_mint();
                    *exchange_issued.lock().unwrap() = Some(token.clone());
                    (
                        axum::http::StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        json!({ "access_token": token }).to_string(),
                    )
                }
            }),
        )
        .route(
            "/user",
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                let user_issued = user_issued.clone();
                async move {
                    if !mock_github_authorized(&headers, &user_issued) {
                        return mock_github_unauthorized();
                    }
                    (
                        axum::http::StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        json!({"id":303,"login":"octocat","avatar_url":"https://example.test/avatar.png"})
                            .to_string(),
                    )
                }
            }),
        )
        .route(
            "/user/emails",
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                let emails = emails.clone();
                let issued = issued.clone();
                async move {
                    if !mock_github_authorized(&headers, &issued) {
                        return mock_github_unauthorized();
                    }
                    (
                        emails.0,
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        emails.1.to_string(),
                    )
                }
            }),
        );
    let server = spawn_test_server(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    );
    (format!("http://{addr}"), server)
}

async fn spawn_mock_github() -> (String, TestServer) {
    // What `read:user` alone cannot answer. The unverified and secondary
    // entries are here because selecting the wrong one is the failure this
    // endpoint exists to make possible.
    spawn_github_stub((
        axum::http::StatusCode::OK,
        json!([
            {"email":"unverified@example.test","primary":false,"verified":false},
            {"email":"secondary@example.test","primary":false,"verified":true},
            {"email":"octocat@example.test","primary":true,"verified":true}
        ]),
    ))
    .await
}

/// The bank as the browser will see it, assembled from the per-problem files
/// under `web/` rather than from `problem-bank/`.
///
/// Reading the generated side on purpose. `gen-problems.py --check` already
/// proves the two agree, and reading the source here would make these tests
/// pass on a tree where the files the browser actually fetches were never
/// regenerated.
///
/// The files are named, and the pages keyed, by each problem's page name, which
/// is all the browser knows. These tests speak in ids, so the id is looked up
/// from the server's own table here: a page no problem claims has no id and
/// fails the lookups below, rather than being quietly matched.
fn id_for_page(page: &str) -> Option<&'static str> {
    codetrial::agent::PROBLEMS
        .iter()
        .find(|problem| problem.variant().page == page)
        .map(|problem| problem.id)
}

fn browser_problem_bank() -> Value {
    let mut problems: Vec<Value> = read_json_dir("web/problems")
        .into_iter()
        .map(|(page, mut problem)| {
            problem["id"] = json!(id_for_page(&page));
            problem
        })
        .collect();
    problems.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    Value::Array(problems)
}

fn browser_judges() -> Value {
    Value::Object(
        read_json_dir("web/judges")
            .into_iter()
            .map(|(page, judge)| (id_for_page(&page).unwrap_or(&page).to_string(), judge))
            .collect(),
    )
}

fn read_json_dir(path: &str) -> std::collections::BTreeMap<String, Value> {
    fs::read_dir(path)
        .unwrap_or_else(|error| panic!("{path} should be generated: {error}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| {
            let id = path.file_stem().unwrap().to_str().unwrap().to_string();
            let text = fs::read_to_string(&path).unwrap();
            let value = serde_json::from_str(&text)
                .unwrap_or_else(|error| panic!("{} should parse: {error}", path.display()));
            (id, value)
        })
        .collect()
}

/// `source` with its whitespace removed and a comma before a closing bracket
/// dropped, so a check against formatted JavaScript reads the same whether or
/// not the formatter wrapped the call and gave it a trailing comma. Whitespace
/// inside a string literal goes too, so a snippet whose meaning rests on a
/// space in a string needs its own check.
fn compact(source: &str) -> String {
    let squashed: String = source.split_whitespace().collect();
    squashed
        .replace(",)", ")")
        .replace(",}", "}")
        .replace(",]", "]")
}

fn source_block<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_index = source
        .find(start)
        .unwrap_or_else(|| panic!("missing {start}"));
    let end_index = source[start_index..]
        .find(end)
        .map(|index| start_index + index)
        .unwrap_or_else(|| panic!("missing {end}"));
    &source[start_index..end_index]
}

/// The top-level arguments of the first `name(` call in `source`, so a nested
/// call's own commas do not split the list. `clamp(parseInt(x, 10) || 45, 10,
/// 90)` has three arguments, not four.
fn call_arguments<'a>(source: &'a str, name: &str) -> Vec<&'a str> {
    let start = source
        .find(name)
        .unwrap_or_else(|| panic!("missing {name} in the browser source"))
        + name.len();
    let mut depth = 1usize;
    let mut arguments = Vec::new();
    let mut argument_start = start;

    // A paren or comma inside a string literal is text, not structure. Today's
    // call has `params.get("duration")` with neither, so this changes nothing;
    // it is here so that adding one later mis-parses nothing rather than
    // shifting the bounds this test is supposed to be pinning.
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (offset, character) in source[start..].char_indices() {
        let at = start + offset;
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == open {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' | '`' => quote = Some(character),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let last = source[argument_start..at].trim();
                    if !last.is_empty() {
                        arguments.push(last);
                    }
                    break;
                }
            }
            ',' if depth == 1 => {
                arguments.push(source[argument_start..at].trim());
                argument_start = at + 1;
            }
            _ => {}
        }
    }
    arguments
}

fn browser_number(source: &str, after: &str, until: char) -> u32 {
    let start = source
        .find(after)
        .unwrap_or_else(|| panic!("missing {after} in the browser source"))
        + after.len();
    let rest = &source[start..];
    let end = rest
        .find(until)
        .unwrap_or_else(|| panic!("{after} is not terminated by {until}"));
    rest[..end]
        .trim()
        .parse()
        .unwrap_or_else(|error| panic!("{after} is not a number: {error}"))
}

/// Agrees to the recording notice the way the browser does, and hands back the
/// interview id `/api/token` then has to be given.
async fn start_interview(client: &reqwest::Client, base: &str, cookie: &str) -> String {
    let response = client
        .post(format!("{base}/api/interviews"))
        .header("cookie", cookie)
        .json(&json!({ "consentVersion": codetrial::recording::CONSENT_VERSION }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "consent should be recorded");
    response.json::<Value>().await.unwrap()["interviewId"]
        .as_str()
        .expect("the response carries the interview id")
        .to_string()
}

fn recording_config() -> codetrial::config::RecordingConfig {
    codetrial::config::RecordingConfig {
        livekit: None,
        gcs_bucket: "codetrial-staging".to_string(),
        gcs_prefix: "codetrial".to_string(),
        drive_id: "0AKfixtureDriveId".to_string(),
        service_account_json: "{}".to_string(),
        max_minutes: 45,
        bitrate: 2000,
        kill_switch: false,
        template_base_url: "https://recording.codetrial.example".to_string(),
    }
}

/// GitHub with a working `/user` and a rate-limited `/user/emails`. The 403
/// body is JSON and parses, which is what makes this failure look like an
/// answer rather than an error to anything that does not check the status.
async fn spawn_rate_limited_github() -> (String, TestServer) {
    spawn_github_stub((
        axum::http::StatusCode::FORBIDDEN,
        json!({"message":"API rate limit exceeded"}),
    ))
    .await
}

/// The `scope` parameter of a redirect location, percent-decoded.
fn query_parameter(location: &str, name: &str) -> Option<String> {
    let query = location.split_once('?')?.1;
    let value = query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{name}=")))?;
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
                decoded.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

/// The router's whole route list, against a fixed allowlist.
///
/// Equality, not containment. The property worth defending is that no route
/// takes media: LiveKit writes the file straight to the staging bucket and
/// CodeTrial never touches the bytes, and "there is no upload route" is a
/// negative that only an exhaustive list can observe. A containment check would
/// pass while an ingest route sat beside the ones it named.
///
/// Read as text out of `src/web/mod.rs` because axum does not hand back the
/// routes it was given. That makes this a tripwire on the source rather than
/// on the running router, which is the trade the whole file already makes for
/// the browser side.
///
/// The ceiling, stated because the name overpromises otherwise: this compares
/// paths. It does not read methods, and it cannot see what a handler does with
/// a body. What it establishes is that the set of paths is the set somebody
/// wrote down, which is what makes "there is no upload route" checkable at
/// all.
/// Every path `web_router` declares, sorted.
///
/// Shared rather than scanned twice: the owner-scope test below needs the same
/// list, and a second scan is a second chance to disagree about what the
/// router holds. The greedy version it started with took the first quote after
/// each `.route(`, which silently aliased both constant-named routes onto the
/// next literal in the file.
fn declared_routes() -> Vec<String> {
    let source = fs::read_to_string("src/web/mod.rs").unwrap();
    let router = source
        .split_once("    Router::new()")
        .expect("web_router builds a Router")
        .1
        .split_once("        .fallback(")
        .expect("the static handler is the fallback")
        .0;

    // This is only exhaustive while every route arrives through `.route(`. A
    // nested or merged router would add paths this scan cannot see, so adding
    // one has to fail here rather than pass quietly.
    for composed in [".nest(", ".nest_service(", ".merge(", ".route_service("] {
        assert!(
            !router.contains(composed),
            "{composed} adds routes this test cannot enumerate; extend it before using one"
        );
    }

    // The first argument of every `.route(` call. Two of them are constants
    // rather than literals, because the contract fixes those paths and
    // `src/recording.rs` owns them, so they are resolved rather than matched.
    let mut routes = router
        .split(".route(")
        .skip(1)
        .map(|rest| {
            let rest = rest.trim_start();
            if rest.starts_with("crate::recording::WEBHOOK_ROUTE") {
                return codetrial::recording::WEBHOOK_ROUTE.to_string();
            }
            if rest.starts_with("crate::recording::REPLAY_ROUTE") {
                return codetrial::recording::REPLAY_ROUTE.to_string();
            }
            rest.trim_start_matches('"')
                .split('"')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect::<Vec<_>>();
    routes.sort();
    routes
}

/// Signs a webhook body the way LiveKit does and posts it.
///
/// The signature covers the exact bytes, so the body is passed as a string and
/// never re-serialized between here and the request.
async fn post_webhook(client: &reqwest::Client, base: &str, body: &str) -> reqwest::Response {
    post_webhook_signed_by(client, base, body, "devkey", b"devsecret").await
}

/// The same, signed by a named project rather than by the primary one.
///
/// A valid signature proves which project sent a webhook, not which project
/// owns the room it names, and those are different questions once a deployment
/// has more than one project.
async fn post_webhook_signed_by(
    client: &reqwest::Client,
    base: &str,
    body: &str,
    api_key: &str,
    api_secret: &[u8],
) -> reqwest::Response {
    post_webhook_authorized_by(
        client,
        base,
        body,
        &webhook_authorization(body, api_key, api_secret),
    )
    .await
}

/// The header LiveKit sends: a JWS whose payload names the project and pins a
/// digest of the body it was signed over.
///
/// Split out from the poster so a test can sign one body and send another,
/// which is the only way to ask whether the handler passes the bytes it
/// received to the verifier or merely checks that some signature parses.
fn webhook_authorization(body: &str, api_key: &str, api_secret: &[u8]) -> String {
    use base64::Engine as _;
    use sha2::Digest as _;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = json!({
        "iss": api_key,
        "nbf": now,
        "exp": now + 300,
        "sha256": base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(body.as_bytes())),
    });
    let header = URL_SAFE_NO_PAD.encode(json!({ "alg": "HS256", "typ": "JWT" }).to_string());
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    let signing_input = format!("{header}.{payload}");
    let mut mac = HmacSha256::new_from_slice(api_secret).unwrap();
    mac.update(signing_input.as_bytes());
    format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

/// The same request with the credential written out by the caller, so a test
/// can post one that is absent, unparsable, or signed over other bytes.
async fn post_webhook_authorized_by(
    client: &reqwest::Client,
    base: &str,
    body: &str,
    authorization: &str,
) -> reqwest::Response {
    let request = client
        .post(format!("{base}{}", codetrial::recording::WEBHOOK_ROUTE))
        .header("content-type", "application/webhook+json")
        .body(body.to_string());
    let request = if authorization.is_empty() {
        request
    } else {
        request.header("authorization", authorization)
    };
    request.send().await.unwrap()
}

/// One replay event on the wire, at a fixed clock.
///
/// Free rather than a closure per test: three tests were writing the same eight
/// lines, so `REPLAY_VERSION` and the timestamp were pinned in three places and
/// a
/// version bump touched all of them.
fn envelope(kind: &str, payload: Value) -> Value {
    json!({
        "v": codetrial::recording::REPLAY_VERSION,
        "kind": kind,
        "at": 1_770_000_000_000i64,
        "payload": payload
    })
}

/// A signed-in account on a server that records, which is where every consent
/// test starts.
async fn recorded_server(
    label: &str,
) -> (
    String,
    TestServer,
    std::path::PathBuf,
    reqwest::Client,
    String,
) {
    let (mut config, cookie, path) = signed_in_web_config(label);
    config.recording = Some(recording_config());
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE users SET email = 'one@example.test', email_verified = 1 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    let (base, server) = spawn_web_server(config).await;
    (base, server, path, http_client(), cookie)
}

/// The same, with a provider a test can watch. Every failure the pipeline has
/// to survive is the provider's, and none can be produced against LiveKit on
/// demand.
async fn recorded_server_with_provider(
    label: &str,
    provider: std::sync::Arc<FakeRecordingProvider>,
) -> (
    String,
    TestServer,
    std::path::PathBuf,
    reqwest::Client,
    String,
) {
    let (mut config, cookie, path) = signed_in_web_config(label);
    config.recording = Some(recording_config());
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE users SET email = 'one@example.test', email_verified = 1 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    let recorder = codetrial::recording::Recorder {
        provider,
        clock: std::sync::Arc::new(codetrial::recording::SystemClock),
        delivery: None,
        config: recording_config(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = spawn_test_server(
        listener,
        codetrial::web::web_service_with_recorder(config, None, recorder),
    );
    (
        format!("http://{addr}"),
        server,
        path,
        http_client(),
        cookie,
    )
}

/// A provider that records what it was asked and always agrees.
#[derive(Default)]
struct FakeRecordingProvider {
    started: std::sync::Mutex<Vec<String>>,
    stopped: std::sync::Mutex<Vec<String>>,
    /// A database to move the row in while `start` is in flight, standing in
    /// for a sweeper retry that won the race. This is the only way to reach the
    /// interleaving from a test: the handler reads its snapshot, the row
    /// changes, and then the handler answers.
    preempt: std::sync::Mutex<Option<std::path::PathBuf>>,
}

impl FakeRecordingProvider {
    fn starts(&self) -> usize {
        self.started.lock().unwrap().len()
    }

    fn stops(&self) -> Vec<String> {
        self.stopped.lock().unwrap().clone()
    }
}

impl codetrial::recording::RecordingProvider for FakeRecordingProvider {
    fn start<'a>(
        &'a self,
        request: &'a codetrial::recording::StartEgress,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            self.started.lock().unwrap().push(request.room_name.clone());
            if let Some(path) = self.preempt.lock().unwrap().as_ref() {
                rusqlite::Connection::open(path)
                    .unwrap()
                    .execute(
                        "
                        UPDATE recordings
                        SET egress_id = 'EG_swept', state = 'recording'
                        WHERE room_name = ?1
                        ",
                        [&request.room_name],
                    )
                    .unwrap();
            }
            Ok("EG_web_fake".to_string())
        })
    }

    fn stop<'a>(
        &'a self,
        egress_id: &'a str,
        _room_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.stopped.lock().unwrap().push(egress_id.to_string());
            Ok(())
        })
    }

    fn active_for_room<'a>(
        &'a self,
        _room_name: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<String>, String>> + Send + 'a>,
    > {
        Box::pin(async move { Ok(None) })
    }
}

/// A second verified account, so cross-account refusals have somebody to be
/// refused for.
async fn second_account(
    client: &reqwest::Client,
    base: &str,
    path: &std::path::Path,
) -> (String, i64) {
    let cookie = record_login(client, base, "two").await;
    let connection = rusqlite::Connection::open(path).unwrap();
    let id: i64 = connection
        .query_row("SELECT id FROM users WHERE login = 'two'", [], |row| {
            row.get(0)
        })
        .unwrap();
    connection
        .execute(
            "UPDATE users SET email = 'two@example.test', email_verified = 1 WHERE id = ?1",
            [id],
        )
        .unwrap();
    (cookie, id)
}

fn interview_rows(path: &std::path::Path) -> Vec<(String, String, i64, Option<i64>)> {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare("SELECT id, consent_version, consent_at, consent_withdrawn_at FROM interviews")
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Whether a quota probe carries a credential this project would accept.
///
/// The token comes out of the `HeaderMap` here and the judgement is
/// `common::livekit_token_matches`, which `tests/cli.rs` makes about the same
/// credential in front of a different double. Sharing the judgement and not the
/// extraction is the split: what a token has to be is one rule, and where it is
/// found is each double's own business.
fn livekit_probe_accepted(
    headers: &axum::http::HeaderMap,
    api_key: &str,
    api_secret: &str,
) -> bool {
    let Some(token) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    common::livekit_token_matches(token, api_key, api_secret)
}

/// The quota stub without a hit counter, which is all most callers need.
async fn spawn_livekit_quota_stub(
    status: axum::http::StatusCode,
    delay: Duration,
    api_key: &str,
    api_secret: &str,
) -> (String, TestServer) {
    let (url, _hits, server) = spawn_counting_quota_stub(status, delay, api_key, api_secret).await;
    (url, server)
}

/// Like [`spawn_livekit_quota_stub`], but counts how many times the project was
/// asked. The count is the whole point of the test below: it is the difference
/// between a pool that probes on a candidate's request and one that already
/// knew.
///
/// `api_key` and `api_secret` are the credentials the caller puts in the pool
/// entry pointing at this stub, and the probe is answered only when its token
/// verifies against them; anything else gets 401, which is what the real
/// `/rtc/validate` answers an unauthenticated GET.
///
/// The check is here because without it this stub answered its status to any
/// caller, and then the probe's credential was the one thing no test in this
/// binary looked at. Dropping `.bearer_auth`, signing with a different
/// project's secret, or minting a token for a project the probe is not asking
/// about would all have left every assertion below intact -- while production
/// learned nothing, since the old pool read every non-429 as "still has
/// minutes". That is the shape of the GitHub stub that
/// answered one profile to any bearer token and let a broken token exchange
/// pass the whole suite.
///
/// Per project rather than merely well-formed, because a pool is more than one
/// project: several of the tests below run two or three stubs at once, each
/// with its own key and secret, so a probe that reached for the wrong entry's
/// credentials is a failure only a per-project check can see.
async fn spawn_counting_quota_stub(
    status: axum::http::StatusCode,
    delay: Duration,
    api_key: &str,
    api_secret: &str,
) -> (
    String,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    TestServer,
) {
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = hits.clone();
    let credentials = std::sync::Arc::new((api_key.to_string(), api_secret.to_string()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = axum::Router::new().route(
        "/rtc/validate",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let counter = counter.clone();
            let credentials = credentials.clone();
            async move {
                tokio::time::sleep(delay).await;

                // Count completion rather than arrival. The background-cache
                // test uses this as the point after which a token may rely on
                // the startup verdict, not merely as evidence that a handler
                // happened to begin receiving a request.
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if !livekit_probe_accepted(&headers, &credentials.0, &credentials.1) {
                    return (axum::http::StatusCode::UNAUTHORIZED, "unauthorized");
                }
                if status == axum::http::StatusCode::TOO_MANY_REQUESTS {
                    (
                        status,
                        "connection minutes limit exceeded. please contact the project owner.",
                    )
                } else {
                    (status, "success")
                }
            }
        }),
    );
    let server = spawn_test_server(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    );
    (format!("ws://127.0.0.1:{}", addr.port()), hits, server)
}

// The tests live in `tests/web/`, declared below. They are modules of this
// target rather than targets of their own: cargo discovers only `tests/*.rs`,
// so a file per subject there would relink the crate once per file, and this
// crate statically links libwebrtc. Helpers and imports stay here, and each
// module opens with `use super::*`.

#[path = "web/token.rs"]
mod token;

#[path = "web/policy.rs"]
mod policy;

#[path = "web/assets.rs"]
mod assets;

#[path = "web/contract.rs"]
mod contract;

#[path = "web/checks.rs"]
mod checks;

#[path = "web/pool.rs"]
mod pool;

#[path = "web/recording.rs"]
mod recording;

#[path = "web/accounts.rs"]
mod accounts;

#[path = "web/routes.rs"]
mod routes;

#[path = "web/tasks.rs"]
mod tasks;

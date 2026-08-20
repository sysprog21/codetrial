use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use codetrial::config::{AgentConfig, DEFAULT_ROOM_PREFIX, DEFAULT_WEB_ADDR, DEFAULT_WEB_DIR};
use codetrial::web::{RoomDispatcher, WebServerConfig};

const DEFAULT_CONFIG_DIR: &str = "config";
const DEFAULT_CONFIG_PATH: &str = "config/codetrial.env.local";
const DEFAULT_ACCOUNT_DB_PATH: &str = "codetrial.db";
const DEFAULT_SESSION_SECRET: &str = "codetrial-local-session";

#[derive(Debug, Default)]
struct CliOptions {
    config_path: Option<String>,
    web_addr: Option<String>,
    web_dir: Option<String>,
    room_prefix: Option<String>,
    duration_min: Option<u32>,
}

/// Every mode this binary has: its name, how many positionals it takes counting
/// the mode word, how it is called, and what runs it. One table and not one
/// list
/// per question, because a mode that exists in the dispatch but not in the
/// arity
/// check is a mode that silently ignores its extra arguments.
const MODES: [(&str, usize, &str, ModeFn); 4] = [
    ("web", 1, "codetrial web [OPTIONS]", |_, options| {
        run_web(options)
    }),
    (
        "run-livekit",
        2,
        "codetrial run-livekit ROOM [OPTIONS]",
        |positionals, options| {
            load_agent_config(&options).and_then(|config| run_livekit(config, &positionals[1]))
        },
    ),
    ("serve", 1, "codetrial serve [OPTIONS]", |_, options| {
        run_serve(options)
    }),
    (
        "check-gemini",
        1,
        "codetrial check-gemini [OPTIONS]",
        |_, options| {
            load_agent_config(&options).and_then(|config| {
                let room_name = format!("{}-smoke", config.room_prefix);
                run_gemini_check(config, &room_name)
            })
        },
    ),
];

/// Positionals include the mode word, and the arity above is what makes
/// indexing
/// past it safe.
type ModeFn = fn(&[String], CliOptions) -> Result<(), String>;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    std::process::exit(run_agent_command(&args));
}

fn run_agent_command(args: &[String]) -> i32 {
    let (positionals, options) = match parse_agent_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    let Some(mode) = positionals.first().map(String::as_str) else {
        eprintln!("usage: codetrial MODE [OPTIONS]");
        return 2;
    };
    let Some(&(_, arity, usage, run)) = MODES.iter().find(|(name, ..)| *name == mode) else {
        eprintln!("unknown agent mode: {mode}");
        return 2;
    };
    if positionals.len() != arity {
        eprintln!("usage: {usage}");
        return 2;
    }

    // Every mode reports failure the same way, so it is reported in one place
    // and each mode returns the message rather than printing it and picking an
    // exit code of its own.
    match run(&positionals, options) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn parse_agent_args(args: &[String]) -> Result<(Vec<String>, CliOptions), String> {
    let mut positionals = Vec::new();
    let mut options = CliOptions::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if !arg.starts_with("--") {
            positionals.push(arg.clone());
            index += 1;
            continue;
        }
        let Some(value) = args.get(index + 1) else {
            return Err(format!("{arg} requires a value"));
        };
        match arg.as_str() {
            "--config" => options.config_path = Some(value.clone()),
            "--web-addr" => options.web_addr = Some(value.clone()),
            "--web-dir" => options.web_dir = Some(value.clone()),
            "--room-prefix" => options.room_prefix = Some(value.clone()),
            "--duration-min" => {
                let duration = value
                    .parse::<u32>()
                    .map_err(|_| format!("invalid --duration-min value: {value}"))?;
                if duration == 0 {
                    return Err(format!("invalid --duration-min value: {value}"));
                }
                options.duration_min = Some(duration);
            }
            _ => return Err(format!("unknown flag: {arg}")),
        }
        index += 2;
    }
    Ok((positionals, options))
}

fn bind_web_listener(values: &BTreeMap<String, String>) -> Result<std::net::TcpListener, String> {
    let addr = value_or(values, "CODETRIAL_WEB_ADDR", DEFAULT_WEB_ADDR);
    let listener = std::net::TcpListener::bind(&addr)
        .map_err(|error| format!("failed to bind {addr}: {error}"))?;
    listener
        .set_nonblocking(true)
        .expect("web listener should become nonblocking");
    Ok(listener)
}

fn run_web(options: CliOptions) -> Result<(), String> {
    let values = load_values(&options)?;

    // The built-in default is a published string, so in production it is not a
    // weak signing key, it is a known one. Refuse rather than mint forgeable
    // session cookies.
    if is_production(&values) && nonempty(&values, "SESSION_SECRET").is_none() {
        return Err(
            "SESSION_SECRET must be set when NODE_ENV=production; the built-in \
                    default is a published value that would let anyone forge a session cookie"
                .to_string(),
        );
    }
    let listener = bind_web_listener(&values)?;

    // Built straight from the values rather than through `load_from_pairs`:
    // this mode serves HTTP and mints LiveKit tokens, and has no use for the
    // Gemini key that a full agent config insists on. Demanding it here turned
    // a web-only deployment into an empty pool, silently.
    let production = is_production(&values);
    let mut pool = codetrial::config::ProviderPool::default();
    if let (Some(url), Some(api_key), Some(api_secret)) = (
        nonempty(&values, "LIVEKIT_URL"),
        nonempty(&values, "LIVEKIT_API_KEY"),
        nonempty(&values, "LIVEKIT_API_SECRET"),
    ) {
        codetrial::config::validate_livekit_url(&url, production)?;
        pool.providers.push(codetrial::config::Provider {
            id: codetrial::config::PRIMARY_PROVIDER_ID.to_string(),
            url,
            api_key,
            api_secret,
            google_api_key: value_or(&values, "GOOGLE_API_KEY", ""),
        });
    }
    extend_pool(
        &mut pool,
        production,
        &provider_dir(&options),
        should_discover_providers(&options),
    );
    let config = WebServerConfig {
        web_dir: PathBuf::from(value_or(&values, "CODETRIAL_WEB_DIR", DEFAULT_WEB_DIR)),
        github_client_id: nonempty(&values, "GITHUB_CLIENT_ID"),
        github_client_secret: nonempty(&values, "GITHUB_CLIENT_SECRET"),
        session_secret: Some(value_or(&values, "SESSION_SECRET", DEFAULT_SESSION_SECRET)),
        db_path: Some(account_db_path(&values)),
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: value_or(&values, "CODETRIAL_ROOM_PREFIX", DEFAULT_ROOM_PREFIX),
        fixed_room_name: nonempty(&values, "INTERVIEW_ROOM_NAME"),
        production: is_production(&values),
        trusted_proxy_hops: trusted_proxy_hops(&values),
        compiler_explorer_enabled: compiler_explorer_enabled(&values),
        pool,
    };

    initialize_accounts(&config)?;

    // Whether this process also hosts interviewers. A full agent config, which
    // is a Gemini key on top of what the web side needs, means yes; without it
    // this is the web half of a split deployment and agents arrive from
    // elsewhere. Deciding it by what is configured, rather than by a flag,
    // keeps a single-host production deployment working out of the box while
    // leaving the split one exactly as it was. No `extend_pool` here on
    // purpose. The dispatcher never looks a provider up: it is handed the one
    // the token was minted from, so a second scan could only introduce a pool
    // that disagrees with the web side's.
    let agent_config = codetrial::config::load_from_pairs(values).ok();
    if agent_config.is_none() {
        eprintln!(
            "no GOOGLE_API_KEY: serving the web side only. Interviews will wait forever unless \
             an agent joins each room from elsewhere."
        );
    }

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should start");
    runtime
        .block_on(async {
            let listener = tokio::net::TcpListener::from_std(listener)?;
            let dispatcher = agent_config.map(|config| {
                Arc::new(LocalDispatcher {
                    config,
                    runtime: tokio::runtime::Handle::current(),
                    live: Arc::default(),
                }) as Arc<dyn RoomDispatcher>
            });
            axum::serve(
                listener,
                codetrial::web::web_service_with_dispatcher(config, dispatcher),
            )
            .await
        })
        .map_err(|error| format!("web server failed: {error}"))
}

/// How many interviews one `codetrial web` process will host agents for at
/// once. Each is a LiveKit room plus a metered Gemini Live session, so an
/// unbounded count is an unbounded bill; a refused dispatch leaves the
/// candidate on the "Waiting" pill, which is bad, but recoverable and visible.
const MAX_CONCURRENT_INTERVIEWS: usize = 16;

/// Runs the interviewer for rooms this process just named, in this process.
///
/// The alternative is a LiveKit agent worker registered against the project,
/// which is the shape LiveKit intends and which survives this process
/// restarting. This is the smaller thing that makes production work: the room
/// name is invented in `/api/token` and never leaves this process, so the
/// process that invented it is the one that can act on it.
struct LocalDispatcher {
    /// Everything an interview needs except which LiveKit project it is in:
    /// that arrives with the room, from the same lookup that minted the token.
    config: AgentConfig,
    runtime: tokio::runtime::Handle,
    live: Arc<Mutex<HashSet<String>>>,
}

impl RoomDispatcher for LocalDispatcher {
    fn ensure_agent(&self, room_name: &str, provider: &codetrial::config::Provider) -> bool {
        {
            let mut live = self.live.lock().unwrap_or_else(|error| error.into_inner());

            // A reload mints a token for the same fixed room in local mode, and
            // two agents in one room evict each other.
            if live.contains(room_name) {
                return true;
            }
            if live.len() >= MAX_CONCURRENT_INTERVIEWS {
                eprintln!(
                    "codetrial dispatch_refused room={room_name} reason=at_capacity limit={MAX_CONCURRENT_INTERVIEWS}"
                );
                return false;
            }
            live.insert(room_name.to_string());
        }
        let config = agent_config_for(&self.config, provider);

        // Released by dropping, not by a line at the end of the task: a panic
        // in the interview would otherwise skip that line and burn the slot for
        // the life of the process, and sixteen of those refuse every interview
        // after them.
        let slot = Slot {
            live: Arc::clone(&self.live),
            room_name: room_name.to_string(),
        };

        // Spawned, not awaited: this runs on the request path, and the task
        // outlives the response by the length of the interview.
        self.runtime.spawn(async move {
            eprintln!("codetrial dispatch room={}", slot.room_name);
            if let Err(error) = codetrial::livekit::run_room(
                &config,
                &slot.room_name,
                codetrial::web::current_epoch_seconds(),
            )
            .await
            {
                eprintln!("codetrial agent_failed room={}: {error}", slot.room_name);
            }
        });
        true
    }
}

/// One interview's claim on this process's capacity, held for as long as the
/// task that owns it.
struct Slot {
    live: Arc<Mutex<HashSet<String>>>,
    room_name: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.live
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.room_name);
    }
}

/// The credential half of [`select_provider`], without the pool lookup, for the
/// caller that was already handed the provider.
fn agent_config_for(base: &AgentConfig, provider: &codetrial::config::Provider) -> AgentConfig {
    let mut config = base.clone();
    config.livekit_url = provider.url.clone();
    config.livekit_api_key = provider.api_key.clone();
    config.livekit_api_secret = provider.api_secret.clone();
    if !provider.google_api_key.is_empty() {
        config.google_api_key = provider.google_api_key.clone();
    }
    config
}

fn run_serve(options: CliOptions) -> Result<(), String> {
    let values = load_values(&options)?;
    let listener = bind_web_listener(&values).map_err(|error| format!("web side {error}"))?;

    // `serve` runs one agent in one room, and `/api/token` only hands out that
    // room while `production` is false. Refusing here beats booting a server
    // that mints rooms nobody is listening in.
    if is_production(&values) {
        return Err("serve is a single-room local mode and cannot run with \
                    NODE_ENV=production; run `codetrial web`, which dispatches an \
                    interviewer per room when GOOGLE_API_KEY is set"
            .to_string());
    }

    let fixed_room_name = nonempty(&values, "INTERVIEW_ROOM_NAME");
    let github_client_id = nonempty(&values, "GITHUB_CLIENT_ID");
    let github_client_secret = nonempty(&values, "GITHUB_CLIENT_SECRET");

    // `serve` refused to run in production above, so the built-in default is
    // always allowed here.
    let session_secret = Some(value_or(&values, "SESSION_SECRET", DEFAULT_SESSION_SECRET));
    let db_path = Some(account_db_path(&values));
    let trusted_proxy_hops = trusted_proxy_hops(&values);
    let mut config =
        codetrial::config::load_from_pairs(values).map_err(|error| error.to_string())?;
    let room_name = fixed_room_name.unwrap_or_else(|| format!("{}-local", config.room_prefix));
    // `serve` refused to run in production above.
    extend_pool(
        &mut config.pool,
        false,
        &provider_dir(&options),
        should_discover_providers(&options),
    );
    let config = select_provider(config, &room_name, false)?;
    let web_config = WebServerConfig {
        web_dir: PathBuf::from(config.web_dir.clone()),
        github_client_id,
        github_client_secret,
        session_secret,
        db_path,
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: config.room_prefix.clone(),
        fixed_room_name: Some(room_name.clone()),
        production: false,
        trusted_proxy_hops,
        compiler_explorer_enabled: config.compiler_explorer_enabled,
        pool: config.pool.clone(),
    };

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should start");
    runtime
        .block_on(run_combined(listener, config, web_config, room_name))
        .map_err(|error| error.to_string())
}

async fn run_combined(
    listener: std::net::TcpListener,
    config: AgentConfig,
    web_config: WebServerConfig,
    room_name: String,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    initialize_accounts(&web_config)?;
    let listener = tokio::net::TcpListener::from_std(listener)?;

    // Built here, not inside the spawn: constructing the service opens the
    // database, migrates it, and sweeps it, all blocking. Inside the task that
    // work parks a runtime worker while the already-bound listener backs up.
    let service = codetrial::web::web_service(web_config);
    let mut web_task = tokio::spawn(async move { axum::serve(listener, service).await });
    let mut agent_task = tokio::spawn(async move {
        run_agent_until_error(
            || {
                let config = config.clone();
                let room_name = room_name.clone();
                async move {
                    codetrial::livekit::run_room(
                        &config,
                        &room_name,
                        codetrial::web::current_epoch_seconds(),
                    )
                    .await
                    .map_err(|error| error.to_string())
                    .or_else(retry_room_end)
                }
            },
            Duration::from_secs(1),
        )
        .await
    });

    // The two sides are not equally fatal, and treating them as if they were is
    // what took the whole product down mid-interview. The web server is what a
    // candidate is looking at: their editor, their test runs, their report. The
    // agent dying is bad, and it is survivable; the web server dying with it
    // turns one failed interview into a dead service for everyone.
    //
    // So the agent gets a bounded number of restarts and the web server keeps
    // serving through them. Bounded, not infinite, because an unrecoverable
    // cause such as a bad API key would otherwise spin forever and look
    // healthy.
    tokio::select! {
        result = &mut web_task => {
            agent_task.abort();
            match result {
                Ok(Ok(())) => Err("web side exited".into()),
                Ok(Err(error)) => Err(format!("web side failed: {error}").into()),
                Err(error) => Err(format!("web side task failed: {error}").into()),
            }
        }
        result = &mut agent_task => {
            // Say it loudly and keep serving. An operator sees this line; a
            // candidate mid-interview sees the interviewer leave, which the
            // browser already surfaces as a banner rather than a frozen page.
            match result {
                Ok(Ok(())) => eprintln!("WARNING: agent side exited; web server still serving"),
                Ok(Err(error)) => eprintln!(
                    "WARNING: agent side failed after {AGENT_RESTART_ATTEMPTS} attempts, \
                     web server still serving: {error}"
                ),
                Err(error) => eprintln!(
                    "WARNING: agent side task failed, web server still serving: {error}"
                ),
            }
            match web_task.await {
                Ok(Ok(())) => Err("web side exited".into()),
                Ok(Err(error)) => Err(format!("web side failed: {error}").into()),
                Err(error) => Err(format!("web side task failed: {error}").into()),
            }
        }
    }
}

/// How many times a failing agent is restarted before `serve` gives up on it.
/// Bounded so an unrecoverable cause, a rejected API key being the obvious one,
/// stops rather than spinning forever behind a healthy-looking process.
const AGENT_RESTART_ATTEMPTS: u32 = 5;

/// Runs the agent, restarting it through transient failures.
///
/// A clean room end is not a failure and never consumes an attempt: `serve`
/// hosts one room and the agent returns each time a candidate leaves. Only real
/// errors do, and they used to be fatal on the first one, which is how a single
/// 404 from duplicate-agent eviction ended an interview.
async fn run_agent_until_error<F, Fut>(
    mut run_once: F,
    restart_delay: Duration,
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    let mut attempts_left = AGENT_RESTART_ATTEMPTS;
    loop {
        match run_once().await {
            // The budget counts CONSECUTIVE failures, and a completed room
            // refills it. Counting cumulatively meant a server up for days
            // exhausted its restarts on five unrelated transient failures
            // spread across a hundred healthy interviews, and then sat there
            // serving rooms no interviewer would ever join. What the bound is
            // actually for is the unrecoverable case, a rejected API key say,
            // which fails immediately every time and never reaches this arm.
            Ok(()) => attempts_left = AGENT_RESTART_ATTEMPTS,
            Err(error) => {
                if attempts_left == 0 {
                    return Err(error);
                }
                attempts_left -= 1;
                eprintln!(
                    "WARNING: agent run failed, restarting ({attempts_left} attempts left): {error}"
                );
            }
        }
        tokio::time::sleep(restart_delay).await;
    }
}

/// An empty room is not a failure: the candidate simply has not opened the tab
/// yet, so `serve` swallows it and waits for the next join.
fn retry_room_end(error: String) -> Result<(), String> {
    if error.contains("room closed before a candidate joined") {
        Ok(())
    } else {
        Err(error)
    }
}

fn run_livekit(config: AgentConfig, room_name: &str) -> Result<(), String> {
    let config = select_provider(config, room_name, true)?;
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should start");
    runtime
        .block_on(codetrial::livekit::run_room(
            &config,
            room_name,
            codetrial::web::current_epoch_seconds(),
        ))
        .map_err(|error| {
            codetrial::gemini::redact_api_key(&error.to_string(), &config.google_api_key)
        })
}

/// The problem and the duration are not arguments: `check-gemini` proves the
/// credentials open a Live session, and the problem it names while doing so is
/// whatever the config picked. `--duration-min` already reaches this through
/// `CODETRIAL_DURATION_MIN`, so a second path for it would only be a second
/// place to disagree.
fn run_gemini_check(config: AgentConfig, room_name: &str) -> Result<(), String> {
    let config = select_provider(config, room_name, true)?;
    let duration_min = config.default_duration_min;
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should start");
    let result = runtime.block_on(async {
        let boot = codetrial::runtime::bootstrap(&config, room_name, None, duration_min);
        let session = codetrial::gemini::open_live_session(&config.google_api_key, &boot).await?;
        session.close().await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(boot)
    });
    let boot = result.map_err(|error| {
        codetrial::gemini::redact_api_key(&error.to_string(), &config.google_api_key)
    })?;
    println!(
        "Gemini setupComplete: model={} voice={} problem={} duration={}min",
        boot.live_model, boot.voice, boot.problem.id, boot.duration_min
    );
    Ok(())
}

/// The room name carries the provider id, so an agent launched separately joins
/// the same LiveKit project the candidate was handed a token for. Refusing
/// beats
/// joining the wrong project and sitting in a room the candidate will never
/// appear in, and the pool this process built is not evidence that the room is
/// wrong: an agent that cannot see the config directory has a pool of one and
/// an
/// unresolvable id, which is exactly the case that has to fail loudly.
///
/// `strict` is off only for `serve`, where the room name is the operator's own
/// `INTERVIEW_ROOM_NAME` and the web half of the same process resolves it from
/// the same pool. Both sides fall back to the primary together there, so a
/// hand-written name that happens to contain a dash cannot split them.
fn select_provider(
    mut config: AgentConfig,
    room_name: &str,
    strict: bool,
) -> Result<AgentConfig, String> {
    if strict
        && let Some(id) = codetrial::config::provider_id_from_room(room_name, &config.room_prefix)
        && config.pool.get(id).is_none()
    {
        return Err(format!(
            "room {room_name} names provider {id}, which has no codetrial.env.{id} in the config directory"
        ));
    }
    let Some(provider) = config.pool.for_room(room_name, &config.room_prefix) else {
        return Ok(config);
    };
    config.livekit_url = provider.url.clone();
    config.livekit_api_key = provider.api_key.clone();
    config.livekit_api_secret = provider.api_secret.clone();
    if !provider.google_api_key.is_empty() {
        config.google_api_key = provider.google_api_key.clone();
    }
    Ok(config)
}

fn load_agent_config(options: &CliOptions) -> Result<AgentConfig, String> {
    let values = load_values(options)?;
    let production = is_production(&values);
    let mut config =
        codetrial::config::load_from_pairs(values).map_err(|error| error.to_string())?;
    extend_pool(
        &mut config.pool,
        production,
        &provider_dir(options),
        should_discover_providers(options),
    );
    Ok(config)
}

fn load_values(options: &CliOptions) -> Result<BTreeMap<String, String>, String> {
    let mut values = std::env::vars().collect::<BTreeMap<_, _>>();
    if std::env::var("CODETRIAL_SKIP_CONFIG").is_err()
        && std::path::Path::new(DEFAULT_CONFIG_PATH).is_file()
    {
        for (key, value) in read_config_file(DEFAULT_CONFIG_PATH)? {
            values.insert(key, value);
        }
    }
    if let Some(path) = &options.config_path {
        for (key, value) in read_config_file(path)? {
            values.insert(key, value);
        }
    }
    if let Some(value) = &options.web_addr {
        values.insert("CODETRIAL_WEB_ADDR".to_string(), value.clone());
    }
    if let Some(value) = &options.web_dir {
        values.insert("CODETRIAL_WEB_DIR".to_string(), value.clone());
    }
    if let Some(value) = &options.room_prefix {
        values.insert("CODETRIAL_ROOM_PREFIX".to_string(), value.clone());
    }
    if let Some(value) = options.duration_min {
        values.insert("CODETRIAL_DURATION_MIN".to_string(), value.to_string());
    }
    Ok(values)
}

fn read_config_file(path: &str) -> Result<Vec<(String, String)>, String> {
    codetrial::config::read_config_file(std::path::Path::new(path))
}

/// Providers live beside the config file the operator named, so
/// `--config /etc/codetrial/prod.env` discovers
/// `/etc/codetrial/codetrial.env.<id>` rather than whatever `config/` happens
/// to sit in the current directory. Both
/// halves of a deployment are launched with the same `--config`, and that is
/// what makes them agree on the pool.
fn provider_dir(options: &CliOptions) -> PathBuf {
    let Some(path) = options.config_path.as_deref() else {
        return PathBuf::from(DEFAULT_CONFIG_DIR);
    };
    match std::path::Path::new(path).parent() {
        // A bare filename has an empty parent, and its siblings are in the
        // working directory, not in `config/`.
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

fn should_discover_providers(options: &CliOptions) -> bool {
    options.config_path.is_some() || std::env::var("CODETRIAL_SKIP_CONFIG").is_err()
}

/// The pool this process routes with. Discovery happens here, once, and never
/// inside `load_from_pairs`: a config loader that reads the current directory
/// gives two processes started from two directories two different pools, and a
/// positional index into those pools then names two different LiveKit projects.
fn extend_pool(
    pool: &mut codetrial::config::ProviderPool,
    production: bool,
    config_dir: &std::path::Path,
    discover: bool,
) {
    if !discover {
        return;
    }
    let (providers, warnings) = codetrial::config::discover_providers(config_dir, production);
    for warning in warnings {
        eprintln!("{warning}");
    }
    pool.providers.extend(providers);
}

/// Refuses to start on a database that will not migrate, before anything is
/// served. The listener is already bound by this point, so the socket exists
/// and its backlog fills, but nothing accepts from it and the process exits
/// non-zero instead. The router runs the same migration again on the connection
/// it keeps, where the only thing it can do about a failure is answer 503.
fn initialize_accounts(config: &WebServerConfig) -> Result<(), String> {
    let Some(login) = codetrial::web::login_config(config) else {
        return Ok(());
    };

    // The router opens the long-lived connection and performs the startup sweep
    // on it; this first pass makes migration failure fatal before the server
    // starts accepting.
    codetrial::web::initialize_account_database(&login.db_path)
        .map_err(|error| format!("account database {}: {error}", login.db_path.display()))?;
    Ok(())
}

fn is_production(values: &BTreeMap<String, String>) -> bool {
    codetrial::config::is_production(values)
}

/// Zero unless the operator states how many proxies front this server, so the
/// default never trusts a forwarded client address.
fn trusted_proxy_hops(values: &BTreeMap<String, String>) -> u32 {
    nonempty(values, "CODETRIAL_TRUSTED_PROXY_HOPS")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn compiler_explorer_enabled(values: &BTreeMap<String, String>) -> bool {
    codetrial::config::compiler_explorer_enabled(
        values
            .get("CODETRIAL_COMPILER_EXPLORER_ENABLED")
            .map(String::as_str),
    )
}

fn nonempty(values: &BTreeMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn value_or(values: &BTreeMap<String, String>, key: &str, default: &str) -> String {
    nonempty(values, key).unwrap_or_else(|| default.to_string())
}

/// One place decides where accounts live, because `web` and `serve` both open
/// the same database and disagreeing about it would split a person's reports.
fn account_db_path(values: &BTreeMap<String, String>) -> PathBuf {
    PathBuf::from(value_or(
        values,
        "CODETRIAL_DB_PATH",
        DEFAULT_ACCOUNT_DB_PATH,
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    /// The capacity slot is released by dropping, so an interview that panics
    /// gives its slot back like one that ends. Sixteen leaked slots would
    /// refuse every interview after them for the life of the process.
    #[tokio::test]
    async fn a_panicking_interview_gives_its_capacity_back() {
        let live = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        live.lock().unwrap().insert("interview-boom".to_string());
        let slot = super::Slot {
            live: Arc::clone(&live),
            room_name: "interview-boom".to_string(),
        };

        let task = tokio::spawn(async move {
            let _held = slot;
            panic!("the interview blew up");
        });
        assert!(task.await.is_err(), "the task must have panicked");

        assert!(
            live.lock().unwrap().is_empty(),
            "a panicking interview must not keep its slot"
        );
    }

    /// The restart loop under a scripted sequence of outcomes: the closure is
    /// handed the attempt number and says what that attempt does. Returns the
    /// error the loop finally gave up with and how many attempts it took.
    async fn attempts_until_error(outcome: fn(usize) -> Result<(), String>) -> (String, usize) {
        let attempts = Arc::new(AtomicUsize::new(0));
        let error = super::run_agent_until_error(
            {
                let attempts = Arc::clone(&attempts);
                move || {
                    let attempts = Arc::clone(&attempts);
                    async move { outcome(attempts.fetch_add(1, Ordering::SeqCst)) }
                }
            },
            Duration::ZERO,
        )
        .await
        .expect_err("the loop must surface a persistent failure");
        (error, attempts.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn agent_loop_restarts_after_clean_room_completion() {
        let (error, attempts) = attempts_until_error(|attempt| {
            if attempt < 2 {
                Ok(())
            } else {
                Err("stop".to_string())
            }
        })
        .await;

        assert_eq!(error, "stop");

        // Two clean completions, then the failure and its restart budget. A
        // clean room end must never consume an attempt: `serve` hosts one room
        // and the agent returns every time a candidate leaves, so counting
        // those would exhaust the budget on an entirely healthy day.
        assert_eq!(attempts, 2 + 1 + super::AGENT_RESTART_ATTEMPTS as usize);
    }

    /// A budget that is never refilled is a slow-motion outage: a server up for
    /// days accumulates unrelated transient failures until the agent stops
    /// restarting, while the web side keeps serving rooms nobody will join.
    #[tokio::test]
    async fn a_completed_room_refills_the_restart_budget() {
        let (error, attempts) = attempts_until_error(|attempt| {
            // Alternates failure and success far more times than the budget
            // allows, which is a flaky day, not a broken deployment, and must
            // not exhaust anything.
            if attempt >= 40 {
                Err("give up".to_string())
            } else if attempt.is_multiple_of(2) {
                Err("transient".to_string())
            } else {
                Ok(())
            }
        })
        .await;

        assert_eq!(error, "give up");

        // Twenty failures survived because each was followed by a clean room.
        // Only the unbroken run at the end spends the budget.
        assert_eq!(attempts, 40 + 1 + super::AGENT_RESTART_ATTEMPTS as usize);
    }

    /// The agent used to die on its first error, and in `serve` that aborted
    /// the web task and exited the process, so one 404 from duplicate-agent
    /// eviction ended the interview and took the editor and the report with it.
    #[tokio::test]
    async fn agent_loop_survives_transient_failures_and_still_gives_up() {
        let (recovered, attempts) = attempts_until_error(|attempt| {
            // Fails once, then runs clean forever, which is what a transient
            // LiveKit or Gemini error looks like.
            if attempt == 0 {
                Err("transient".to_string())
            } else if attempt < 4 {
                Ok(())
            } else {
                Err("done".to_string())
            }
        })
        .await;

        // It recovered from the first error rather than surfacing it, and the
        // budget it spent doing so is not refunded by the clean runs between.
        assert_eq!(recovered, "done");
        assert!(attempts > 4);
    }

    #[tokio::test]
    async fn agent_loop_retries_after_room_closes_before_candidate() {
        let (error, attempts) = attempts_until_error(|attempt| {
            if attempt == 0 {
                super::retry_room_end("room closed before a candidate joined".to_string())
            } else {
                Err("stop".to_string())
            }
        })
        .await;

        assert_eq!(error, "stop");
        // One swallowed room-close, then the failure and its restart budget.
        assert_eq!(attempts, 1 + 1 + super::AGENT_RESTART_ATTEMPTS as usize);
    }

    fn agent_config(pool: codetrial::config::ProviderPool) -> super::AgentConfig {
        let mut config = codetrial::config::load_from_pairs([
            ("LIVEKIT_URL", "wss://primary.example"),
            ("LIVEKIT_API_KEY", "primary-key"),
            ("LIVEKIT_API_SECRET", "primary-secret"),
            ("GOOGLE_API_KEY", "primary-google"),
        ])
        .unwrap();
        config.pool = pool;
        config
    }

    fn provider(id: &str) -> codetrial::config::Provider {
        codetrial::config::Provider {
            id: id.to_string(),
            url: format!("wss://{id}.example"),
            api_key: format!("{id}-key"),
            api_secret: format!("{id}-secret"),
            google_api_key: format!("{id}-google"),
        }
    }

    /// The whole point of writing the provider id into the room name: an agent
    /// started separately has to land in the project the candidate was given a
    /// token for, and has to say so when it cannot.
    #[test]
    fn a_room_name_routes_the_agent_to_the_provider_it_names() {
        let pool = codetrial::config::ProviderPool {
            providers: vec![
                provider(codetrial::config::PRIMARY_PROVIDER_ID),
                provider("eu"),
            ],
        };

        let config =
            super::select_provider(agent_config(pool.clone()), "interview-eu-a1b2c3d4", true)
                .expect("a configured provider should resolve");
        assert_eq!(config.livekit_url, "wss://eu.example");
        assert_eq!(config.livekit_api_secret, "eu-secret");
        assert_eq!(config.google_api_key, "eu-google");

        // No segment: what a single-provider deployment mints, and what a
        // hand-written INTERVIEW_ROOM_NAME usually looks like.
        let config =
            super::select_provider(agent_config(pool), "interview-a1b2c3d4", true).unwrap();
        assert_eq!(config.livekit_url, "wss://primary.example");
    }

    /// Falling back to the primary here is how an agent joins the wrong project
    /// and sits in a room the candidate never appears in, for the whole
    /// interview. A pool of one is exactly what a failed discovery produces, so
    /// pool size is not evidence about the room.
    #[test]
    fn an_unconfigured_provider_id_is_refused_not_guessed() {
        for providers in [
            vec![provider(codetrial::config::PRIMARY_PROVIDER_ID)],
            vec![
                provider(codetrial::config::PRIMARY_PROVIDER_ID),
                provider("us"),
            ],
        ] {
            let pool = codetrial::config::ProviderPool { providers };
            // Not `expect_err`: `AgentConfig` deliberately has no `Debug`.
            let Err(error) =
                super::select_provider(agent_config(pool.clone()), "interview-eu-a1b2c3d4", true)
            else {
                panic!("an unknown provider id should refuse");
            };
            assert!(error.contains("eu"), "{error}");

            // `serve` runs both halves from one pool, so they fall back
            // together and a dash in a fixed room name is not an error.
            let config =
                super::select_provider(agent_config(pool), "interview-eu-a1b2c3d4", false).unwrap();
            assert_eq!(config.livekit_url, "wss://primary.example");
        }
    }

    #[test]
    fn bare_config_paths_discover_providers_from_the_current_directory() {
        let options = super::CliOptions {
            config_path: Some("prod.env".to_string()),
            ..Default::default()
        };
        assert_eq!(super::provider_dir(&options), std::path::PathBuf::from("."));
    }

    #[test]
    fn explicit_config_keeps_provider_discovery_when_auto_config_is_skipped() {
        let options = super::CliOptions {
            config_path: Some("/etc/codetrial/prod.env".to_string()),
            ..Default::default()
        };
        assert!(super::should_discover_providers(&options));
    }
}

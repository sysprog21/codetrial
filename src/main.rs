use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use codetrial::config::{
    AgentConfig, DEFAULT_DURATION_MIN, DEFAULT_ROOM_PREFIX, DEFAULT_WEB_ADDR, DEFAULT_WEB_DIR,
};
use codetrial::web::{RoomDispatcher, WebServerConfig};

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
const MODES: [(&str, usize, &str, ModeFn); 3] = [
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
    // Answered before parsing, and from the raw arguments. `--help` beside a
    // flag that is missing its value is still a request for help, and a parse
    // error is not an answer to it. Resolving it here is also why `CliOptions`
    // carries no help field: nothing downstream ever sees one.
    if args
        .iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--help" || arg == "-h")
    {
        print_help(args.iter().find_map(|arg| {
            MODES
                .iter()
                .find(|(name, ..)| name == arg)
                .map(|(_, _, usage, _)| *usage)
        }));
        return 0;
    }
    let (mut positionals, options) = match parse_agent_args(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    // No console survives to show a usage line on double-click; default to `web`.
    if positionals.is_empty() {
        positionals.push("web".to_string());
    }
    let mode = positionals[0].as_str();
    let Some(&(_, arity, usage, run)) = MODES.iter().find(|(name, ..)| *name == mode) else {
        // The names, not just the refusal. A mode that was removed reaches this
        // line as an ordinary typo, and "unknown agent mode: serve" on its own
        // leaves the reader to guess what replaced it. `MODES` is the list, so
        // it cannot go stale the way a sentence naming one of them would.
        eprintln!(
            "unknown agent mode: {mode}\nmodes: {}",
            MODES
                .iter()
                .map(|(name, ..)| *name)
                .collect::<Vec<_>>()
                .join(", ")
        );
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

/// Positionals and options, in one pass over the arguments.
///
/// Driven by an iterator rather than an index. The index form needed a manual
/// `index += 1` or `+= 2` on every path, which is one edit away from a loop
/// that never advances: mutation testing hangs it by turning that `+=` into
/// `-=`. Nothing here can fail to make progress, because `next` is the only way
/// forward and it is unconditional.
fn parse_agent_args(args: &[String]) -> Result<(Vec<String>, CliOptions), String> {
    let mut positionals = Vec::new();
    let mut options = CliOptions::default();
    let mut rest = args.iter().peekable();
    while let Some(arg) = rest.next() {
        // Everything after a bare `--` is a positional, whatever it looks like.
        // The escape hatch the separated form below needs: without it there is
        // no way to name a file whose own name starts with a dash.
        if arg == "--" {
            positionals.extend(rest.cloned());
            break;
        }
        if !arg.starts_with("--") {
            positionals.push(arg.clone());
            continue;
        }

        // `--flag=value` or `--flag value`. The attached form is what makes a
        // dash-prefixed value expressible at all, because in the separated form
        // a flag in the value slot is a missing value and not the value:
        // without that rule `--config --help` reports a configuration file
        // named `--help` instead of printing help. `next_if` is what leaves it
        // where it is, to be read as the flag it is on the next turn.
        let (flag, value) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (
                arg.as_str(),
                rest.next_if(|next| !next.starts_with('-')).cloned(),
            ),
        };
        let Some(value) = value else {
            return Err(format!("{flag} requires a value"));
        };
        match flag {
            "--config" => options.config_path = Some(value),
            "--web-addr" => options.web_addr = Some(value),
            "--web-dir" => options.web_dir = Some(value),
            "--room-prefix" => options.room_prefix = Some(value),
            "--duration-min" => {
                let duration = value
                    .parse::<u32>()
                    .map_err(|_| format!("invalid --duration-min value: {value}"))?;
                if duration == 0 {
                    return Err(format!("invalid --duration-min value: {value}"));
                }
                options.duration_min = Some(duration);
            }
            _ => return Err(format!("unknown flag: {flag}")),
        }
    }
    Ok((positionals, options))
}

/// Help, for one mode or for the binary.
///
/// One printer rather than two. The mode-specific half was a `println!` beside
/// a call to the general half, which is a shape that drifts: the options are
/// the same options either way, and the only difference is whether the modes
/// are listed above them.
fn print_help(usage: Option<&str>) {
    match usage {
        Some(usage) => println!("usage: {usage}"),
        None => {
            println!("usage: codetrial MODE [OPTIONS]\n\nModes:");
            for (_, _, usage, _) in MODES {
                println!("  {usage}");
            }
            println!("\nRun `codetrial MODE --help` for mode-specific usage.");
        }
    }
    println!(
        "\nOptions:\n  --config PATH        Use PATH instead of config/codetrial.env.local\n  --web-addr ADDR      Listen on ADDR\n  --web-dir PATH       Serve files from PATH\n  --room-prefix PREFIX Prefix generated room names\n  --duration-min MIN   Set the interview duration\n  -h, --help           Show this help\n\nEvery option also takes `--flag=value`, which is the only way to pass a value\nthat starts with a dash. `--` ends the options."
    );
}

fn bind_web_listener(addr: &str) -> Result<std::net::TcpListener, String> {
    let listener = std::net::TcpListener::bind(addr)
        .map_err(|error| format!("failed to bind {addr}: {error}"))?;
    listener
        .set_nonblocking(true)
        .expect("web listener should become nonblocking");
    Ok(listener)
}

fn run_web(options: CliOptions) -> Result<(), String> {
    if is_cold_start(&options) {
        // Falls through: once `serve_setup` returns, the config exists, so
        // the rest of this function is an ordinary launch — same process.
        serve_setup(&options)?;
    }
    let values = load_values(&options)?;

    // The built-in default is a published string, so in production it is not a
    // weak signing key, it is a known one. Refuse rather than mint forgeable
    // session cookies.
    if is_production(&values)
        && value_or(&values, "SESSION_SECRET", DEFAULT_SESSION_SECRET) == DEFAULT_SESSION_SECRET
    {
        return Err(
            "SESSION_SECRET must be set when NODE_ENV=production; the built-in \
                    default is a published value that would let anyone forge a session cookie"
                .to_string(),
        );
    }
    for warning in relaxed_for_local_use(&values) {
        eprintln!("{warning}");
    }

    // Beside the other startup warnings and not next to the dispatcher that
    // consumes it. A cap the operator wrote and this cannot honor is worth
    // saying before the first thing that can fail: read late, a bad bind or a
    // half-configured pool ends the process first and the operator never learns
    // the number in their config file was ignored.
    let max_concurrent = read_max_concurrent(&values);

    let production = is_production(&values);
    let pool = web_provider_pool(&values, &options, production)?;

    // After the pool is complete, so the "is this project in the pool" check
    // sees the pool the server will actually serve, and before the listener
    // does any work: a half-configured recording block is a startup failure,
    // never a surprise at the moment a candidate's interview ends.
    let recording = codetrial::config::load_recording(
        &values,
        &pool,
        interview_duration_min(&values),
        production,
    )
    .map_err(|error| error.to_string())?;
    recording_needs_verified_identity(
        recording.is_some(),
        nonempty(&values, "GITHUB_CLIENT_ID").is_some()
            && nonempty(&values, "GITHUB_CLIENT_SECRET").is_some(),
    )?;
    recording_can_deliver(recording.as_ref())?;

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
        recording,
        pool,
        probe_provider_quota: true,
    };

    initialize_accounts(&config)?;
    let listener = bind_web_listener(&value_or(&values, "CODETRIAL_WEB_ADDR", DEFAULT_WEB_ADDR))?;

    // The other half of the `SESSION_SECRET` guard above, and the half that
    // does not depend on the operator having said anything.
    //
    // After the bind rather than before it, because the address may be a
    // hostname: `CODETRIAL_WEB_ADDR=interviews.example:3000` is resolved by
    // `ToSocketAddrs` and only the socket knows what it landed on. That puts it
    // beside `initialize_accounts`, which refuses after binding for the same
    // reason and with the same consequence: the socket exists and its backlog
    // fills, nothing accepts from it, and the process exits non-zero.
    //
    // Fails closed when the socket cannot name itself, which a just-bound
    // listener does not do. Skipping the check on an unreadable address would
    // make an impossible kernel answer the one way past a security guard, and
    // refusing to start is the cheaper half of that trade.
    let bound = listener
        .local_addr()
        .map_err(|error| format!("bound listener has no address to check: {error}"))?;
    if let Some(refusal) = published_secret_refusal(&values, bound) {
        return Err(refusal);
    }
    // Same reason `serve_setup` prints this: say where to go.
    println!("codetrial: open http://{bound} in your browser");

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
                Arc::new(codetrial::dispatch::LocalDispatcher {
                    config,
                    runtime: tokio::runtime::Handle::current(),
                    live: Arc::default(),
                    max_concurrent,
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

/// The pool this deployment will serve.
///
/// Built straight from the values rather than through `load_from_pairs`: the
/// web mode serves HTTP and mints LiveKit tokens, and has no use for the Gemini
/// key that a full agent config insists on. Demanding it here turned a web-only
/// deployment into an empty pool, silently.
fn web_provider_pool(
    values: &BTreeMap<String, String>,
    options: &CliOptions,
    production: bool,
) -> Result<codetrial::config::ProviderPool, String> {
    let mut pool = codetrial::config::ProviderPool::default();
    let (url, api_key, api_secret) = match (
        nonempty(values, "LIVEKIT_URL"),
        nonempty(values, "LIVEKIT_API_KEY"),
        nonempty(values, "LIVEKIT_API_SECRET"),
    ) {
        (Some(url), Some(api_key), Some(api_secret)) => (url, api_key, api_secret),

        // Names the file that was actually read. A fixed `config/` in this
        // message sends anyone running with `--config` or a bare
        // `./codetrial.env.local` to edit a file the server never opened.
        _ => {
            return Err(format!(
                "missing required LiveKit credentials: set LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET in {}",
                primary_config_path(options)?.display()
            ));
        }
    };
    codetrial::config::validate_livekit_url(&url, production)?;
    pool.providers.push(codetrial::config::Provider {
        id: codetrial::config::PRIMARY_PROVIDER_ID.to_string(),
        url,
        api_key,
        api_secret,
        google_api_key: value_or(values, "GOOGLE_API_KEY", ""),
    });
    extend_pool(
        &mut pool,
        production,
        &provider_dir(options),
        &value_or(values, codetrial::config::PROVIDER_ORDER_KEY, ""),
    );
    Ok(pool)
}

/// The solo self-serve cold start: no `--config` given and none of the
/// searched paths holds a file. A named-but-missing `--config` is an
/// operator's mistake, not this.
///
/// The search order comes from `primary_config_path` rather than being
/// written out again here. Spelled twice it was free to drift, and the drift
/// is silent in the worst direction: a Setup page in front of someone whose
/// config the rest of the process is about to read.
fn is_cold_start(options: &CliOptions) -> bool {
    options.config_path.is_none() && primary_config_path(options).is_err()
}

/// Serves Setup until a submission writes `codetrial.env.local`, then
/// returns so `run_web` continues as an ordinary launch. Drop-and-rebind,
/// not a swappable router: the gap is milliseconds, worth one retry.
fn serve_setup(options: &CliOptions) -> Result<(), String> {
    let listener = bind_web_listener(options.web_addr.as_deref().unwrap_or(DEFAULT_WEB_ADDR))?;
    // The window stays open now, but still needs to say where to go.
    if let Ok(bound) = listener.local_addr() {
        println!("codetrial: open http://{bound} in your browser to continue setup");
    }
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should start");
    runtime
        .block_on(async {
            let ready = Arc::new(tokio::sync::Notify::new());
            let listener = tokio::net::TcpListener::from_std(listener)?;
            axum::serve(listener, codetrial::web::setup_service(ready.clone()))
                .with_graceful_shutdown(async move { ready.notified().await })
                .await
        })
        .map_err(|error| format!("web server failed: {error}"))
}

fn run_livekit(config: AgentConfig, room_name: &str) -> Result<(), String> {
    let config = select_provider(config, room_name)?;
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
    let config = select_provider(config, room_name)?;
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

    // Said out loud because `make check` runs this, and a green run reads as
    // "the stack works". It opens a Gemini Live session and never authenticates
    // against LiveKit, so it passes with a wrong `LIVEKIT_API_SECRET`. Someone
    // already spent an afternoon treating this line as proof it was right.
    println!("Not checked here: LiveKit credentials. This never calls LiveKit.");
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
fn select_provider(mut config: AgentConfig, room_name: &str) -> Result<AgentConfig, String> {
    if let Some(id) = codetrial::config::provider_id_from_room(room_name, &config.room_prefix)
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
    let provider_order = value_or(&values, codetrial::config::PROVIDER_ORDER_KEY, "");
    let mut config =
        codetrial::config::load_from_pairs(values).map_err(|error| error.to_string())?;
    extend_pool(
        &mut config.pool,
        production,
        &provider_dir(options),
        &provider_order,
    );
    Ok(config)
}

fn load_values(options: &CliOptions) -> Result<BTreeMap<String, String>, String> {
    let mut values = std::env::vars().collect::<BTreeMap<_, _>>();
    let path = primary_config_path(options)?;
    for (key, value) in codetrial::config::read_config_file(&path)? {
        values.insert(key, value);
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

fn primary_config_path(options: &CliOptions) -> Result<PathBuf, String> {
    if let Some(path) = &options.config_path {
        let path = PathBuf::from(path);
        if !path.is_file() {
            return Err(format!(
                "required configuration file is missing: {}",
                path.display()
            ));
        }
        return Ok(path);
    }
    // The working directory first, which is a checkout's `config/` and an
    // operator's deployment directory, then the folder the executable sits in.
    // A release binary is unpacked into a folder of its own and its config is
    // written there, so it has to be findable from a shortcut or a terminal
    // opened somewhere else, not only from a double-click.
    for path in [
        PathBuf::from(DEFAULT_CONFIG_PATH),
        PathBuf::from("codetrial.env.local"),
        codetrial::exe_dir().join("codetrial.env.local"),
    ] {
        if path.is_file() {
            return Ok(path);
        }
    }

    // The keys, not a file to copy. `config/codetrial.env.example` exists in a
    // checkout and nowhere else: the release archives hold the executable and
    // nothing beside it, so telling someone who just unpacked one to copy it
    // sends them looking for a file they were never given. Naming what the file
    // must contain is an instruction both audiences can act on.
    Err(format!(
        "required configuration file is missing: ./{DEFAULT_CONFIG_PATH}, \
         ./codetrial.env.local, or codetrial.env.local beside the executable; \
         write one with LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET \
         (config/codetrial.env.example lists the optional keys in a checkout)"
    ))
}

/// Providers live beside the config file the operator named, so
/// `--config /etc/codetrial/prod.env` discovers
/// `/etc/codetrial/codetrial.env.<id>` rather than whatever `config/` happens
/// to sit in the current directory. Both
/// halves of a deployment are launched with the same `--config`, and that is
/// what makes them agree on the pool.
fn provider_dir(options: &CliOptions) -> PathBuf {
    // The search order comes from primary_config_path rather than being written
    // out again here. Spelled twice, it was free to drift, and the drift is
    // silent: the server keeps reading its own config while the pool quietly
    // collapses to the primary project.
    //
    // A named file still decides, even when it is missing. The operator said
    // where the config lives, and load_values is what refuses a path that is
    // not there; answering with a different directory would be this function
    // second-guessing that.
    let path = options
        .config_path
        .as_deref()
        .map(PathBuf::from)
        .or_else(|| primary_config_path(options).ok())
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
    match path.parent() {
        // A bare filename has an empty parent, and its siblings are in the
        // working directory, not in `config/`.
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// The pool this process routes with. Discovery happens here, once, and never
/// inside `load_from_pairs`: a config loader that reads the current directory
/// gives two processes started from two directories two different pools, and a
/// positional index into those pools then names two different LiveKit projects.
fn extend_pool(
    pool: &mut codetrial::config::ProviderPool,
    production: bool,
    config_dir: &std::path::Path,
    order: &str,
) {
    let (providers, warnings) = codetrial::config::discover_providers(config_dir, production);
    for warning in warnings {
        eprintln!("{warning}");
    }
    pool.providers.extend(providers);

    // Outside the `discover` guard: an order naming the environment provider is
    // still an order, and a deployment with discovery off should not silently
    // ignore the one it was given.
    for warning in codetrial::config::order_providers(&mut pool.providers, order) {
        eprintln!("{warning}");
    }

    // Said once, at startup, in rotation order. Every provider question asked
    // of a running server so far has been answerable only by reading the config
    // directory and the argv back to itself and guessing which won.
    if !pool.providers.is_empty() {
        let (summary, warnings) = codetrial::config::pool_summary(&pool.providers);
        eprintln!("{summary}");
        for warning in warnings {
            eprintln!("WARNING: {warning}");
        }
    }
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

/// The dispatcher cap, with anything unusable about it said out loud.
///
/// One helper for both modes: a warning printed by web and swallowed by serve
/// would be the same misconfiguration reported twice differently.
fn read_max_concurrent(values: &BTreeMap<String, String>) -> usize {
    let mut warnings = Vec::new();
    let max_concurrent = codetrial::config::max_concurrent_interviews(values, &mut warnings);
    for warning in &warnings {
        eprintln!("{warning}");
    }
    max_concurrent
}

fn is_production(values: &BTreeMap<String, String>) -> bool {
    codetrial::config::is_production(values)
}

/// Zero unless the operator states how many proxies front this server, so the
/// default never trusts a forwarded client address.
/// Recording without an OAuth app is a server that refuses every interview.
///
/// The delivery target is the primary verified address GitHub returns, and
/// without the OAuth flow every account is self-declared, so `/api/token`
/// answers 403 to all of them. Said at startup, where an operator can act on
/// it, rather than one candidate at a time.
///
/// `oauth` is both credentials, not just the id. `login_config` enables OAuth
/// only when it has the pair, so an id with no secret is exactly as unable to
/// verify anyone as no id at all, and would have started a server that refused
/// every interview while looking configured.
/// A deployment that records has to be able to hand the file over.
///
/// The key is parsed here, before the listener does any work, because the
/// alternative is a server that records interviews and discovers at the first
/// delivery that it can never sign for one. Configuration already checked the
/// shape; this is the credential itself.
fn recording_can_deliver(
    recording: Option<&codetrial::config::RecordingConfig>,
) -> Result<(), String> {
    let Some(recording) = recording else {
        return Ok(());
    };
    codetrial::delivery::GoogleDelivery::new(
        &recording.service_account_json,
        &recording.gcs_bucket,
        &recording.drive_id,
        std::sync::Arc::new(|| 0),
    )
    .map(|_| ())
    .map_err(|error| format!("CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON: {error}"))
}

fn recording_needs_verified_identity(recording: bool, oauth: bool) -> Result<(), String> {
    if recording && !oauth {
        return Err(
            "CODETRIAL_RECORDING_ENABLED=true needs GITHUB_CLIENT_ID and \
                    GITHUB_CLIENT_SECRET: a recording is delivered to the verified address \
                    GitHub returns, and without the OAuth app every account is self-declared \
                    and every interview would be refused"
                .to_string(),
        );
    }
    Ok(())
}

/// The configured interview length, which is what a recording's maximum
/// duration has to clear. Read here rather than through `load_from_pairs`
/// because `codetrial web` deliberately runs without a full agent config.
fn interview_duration_min(values: &BTreeMap<String, String>) -> u32 {
    nonempty(values, "CODETRIAL_DURATION_MIN")
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_DURATION_MIN)
}

fn trusted_proxy_hops(values: &BTreeMap<String, String>) -> u32 {
    nonempty(values, "CODETRIAL_TRUSTED_PROXY_HOPS")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// What a `codetrial web` start without `NODE_ENV=production` is allowing, in
/// the operator's words rather than the code's.
///
/// Every guard this reports on is keyed on `NODE_ENV`, so the one mistake none
/// of them can catch is the deployment that sets every credential correctly and
/// never sets `NODE_ENV` at all. The refusal in `run_web` fires only once the
/// operator has already said this is production; a server that was meant to be
/// production and does not say so gets the local defaults and no complaint.
/// This is the only signal it ever gets.
///
/// `config::is_production` matches the exact string, so a `NODE_ENV=Production`
/// typo lands here too, which is the other way a deployment ends up local
/// without meaning to.
fn relaxed_for_local_use(values: &BTreeMap<String, String>) -> Vec<String> {
    if is_production(values) {
        return Vec::new();
    }

    let mut warnings = vec![
        "NODE_ENV is not production: the page's CSP allows loopback origins and a \
         plaintext ws:// LiveKit URL is accepted. Set NODE_ENV=production to deploy."
            .to_string(),
    ];
    if nonempty(values, "SESSION_SECRET").is_none() {
        warnings.push(format!(
            "SESSION_SECRET is unset: signing session cookies with the built-in \
                 {DEFAULT_SESSION_SECRET:?}, which is published in this repository and lets \
                 anyone forge a session."
        ));
    }
    warnings
}

/// Why the built-in `SESSION_SECRET` may not sign cookies on `bound`, or `None`
/// where it may.
///
/// Split out for the reason `relaxed_for_local_use` is: the caller can only
/// return the string, and a refusal reachable solely by starting a process that
/// otherwise never exits is a branch no test can assert on without hanging.
///
/// `NODE_ENV` is a claim; the address this process bound is a fact. A server
/// other machines can reach is not the local run the published default exists
/// for, whether or not anyone remembered to say so, which is the mistake the
/// warning in `relaxed_for_local_use` can name but not prevent.
///
/// Unspecified addresses (`0.0.0.0`, `::`) are not loopback and must not be
/// treated as such: binding every interface is how a container serves the
/// world.
///
/// `to_canonical` before the test, because `Ipv6Addr::is_loopback` is true of
/// `::1` and of nothing else: a dual-stack socket that landed on
/// `::ffff:127.0.0.1`, which is a name for the same interface, would otherwise
/// be refused an address only this machine can reach.
fn published_secret_refusal(
    values: &BTreeMap<String, String>,
    bound: std::net::SocketAddr,
) -> Option<String> {
    // What the cookie signer will actually use, not whether the key was
    // mentioned. `value_or` trims, so `SESSION_SECRET=" codetrial-local-session
    // "` is the published key spelled with whitespace rather than a secret of
    // the operator's own, and a guard that only asked whether the variable was
    // set would wave it through.
    if value_or(values, "SESSION_SECRET", DEFAULT_SESSION_SECRET) != DEFAULT_SESSION_SECRET {
        return None;
    }

    // Two ways to be reachable, and the second is the one a bind address cannot
    // see: a server on loopback behind nginx or a tunnel is as public as one on
    // `0.0.0.0`. `CODETRIAL_TRUSTED_PROXY_HOPS` is the operator saying requests
    // arrive through something in front, which is the same admission.
    let reason = if !bound.ip().to_canonical().is_loopback() {
        format!("bind {bound}")
    } else if trusted_proxy_hops(values) > 0 {
        "serve from behind a declared proxy".to_string()
    } else {
        return None;
    };

    Some(format!(
        "SESSION_SECRET must be set to {reason}: {DEFAULT_SESSION_SECRET:?} is published in \
         this repository, so every session cookie this server signs would be forgeable by \
         anyone who can reach it"
    ))
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
    fn values(pairs: &[(&str, &str)]) -> super::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    /// A hostname, not a literal socket address. `bind_web_listener` hands the
    /// string to `ToSocketAddrs`, which resolves it, and `CODETRIAL_WEB_ADDR`
    /// has always accepted one. This used to be guarded in `config`, where the
    /// value was parsed into a field only `serve` read; with the field gone,
    /// the guard belongs on the one function that still takes the string.
    #[test]
    fn a_hostname_web_address_binds() {
        let listener =
            super::bind_web_listener("localhost:0").expect("a hostname address should bind");
        assert!(
            listener
                .local_addr()
                .expect("a bound listener has an address")
                .ip()
                .is_loopback()
        );
    }

    fn addr(text: &str) -> std::net::SocketAddr {
        text.parse().expect("test address should parse")
    }

    /// The published default is tolerated on loopback and nowhere else, and the
    /// two unspecified addresses are the ones a container binds: neither is
    /// loopback, and reading them as local is how the guard would miss the
    /// deployment it exists for.
    #[test]
    fn the_published_session_secret_is_confined_to_loopback() {
        for local in [
            "127.0.0.1:3000",
            "127.0.0.53:3000",
            "[::1]:3000",
            // A dual-stack socket's spelling of the first one.
            "[::ffff:127.0.0.1]:3000",
        ] {
            assert_eq!(
                super::published_secret_refusal(&values(&[]), addr(local)),
                None,
                "{local} is loopback and needs no secret"
            );
        }

        for public in ["0.0.0.0:3000", "[::]:3000", "192.168.1.10:3000"] {
            let refusal = super::published_secret_refusal(&values(&[]), addr(public))
                .unwrap_or_else(|| panic!("{public} must be refused"));
            assert!(refusal.contains(public), "{refusal}");
            assert!(refusal.contains("SESSION_SECRET must be set"), "{refusal}");
        }
    }

    /// A secret of the operator's own is the whole point: setting it must lift
    /// the restriction rather than merely change the message.
    #[test]
    fn a_real_session_secret_is_allowed_on_any_address() {
        assert_eq!(
            super::published_secret_refusal(
                &values(&[("SESSION_SECRET", "a-real-secret")]),
                addr("0.0.0.0:3000")
            ),
            None
        );
    }

    /// Naming the published value is not setting a secret. A guard that asked
    /// only whether `SESSION_SECRET` was present would take the string this
    /// repository publishes as proof that it is not being used, and whitespace
    /// around it changes nothing, because the signer trims before it signs.
    #[test]
    fn spelling_out_the_published_default_does_not_count_as_setting_one() {
        for spelling in [super::DEFAULT_SESSION_SECRET, "  codetrial-local-session  "] {
            assert!(
                super::published_secret_refusal(
                    &values(&[("SESSION_SECRET", spelling)]),
                    addr("0.0.0.0:3000")
                )
                .is_some(),
                "{spelling:?} is the published key, not a secret"
            );
        }
    }

    /// A bind address cannot see a reverse proxy. Declaring hops is the
    /// operator saying requests reach this process from somewhere else, which
    /// makes a loopback socket as public as the proxy in front of it.
    #[test]
    fn a_declared_proxy_makes_loopback_public_too() {
        let refusal = super::published_secret_refusal(
            &values(&[("CODETRIAL_TRUSTED_PROXY_HOPS", "1")]),
            addr("127.0.0.1:3000"),
        )
        .expect("loopback behind a declared proxy must be refused");
        assert!(refusal.contains("behind a declared proxy"), "{refusal}");

        assert_eq!(
            super::published_secret_refusal(
                &values(&[("CODETRIAL_TRUSTED_PROXY_HOPS", "0")]),
                addr("127.0.0.1:3000")
            ),
            None,
            "no declared proxy is the local run the default exists for"
        );
    }

    /// The deployment this whole warning exists for: every credential set, and
    /// `NODE_ENV` forgotten. Nothing else in the startup path says a word about
    /// it, so if this stops firing the misconfiguration goes back to silent.
    #[test]
    fn a_start_without_node_env_says_what_it_relaxed() {
        let warnings = super::relaxed_for_local_use(&values(&[
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("GITHUB_CLIENT_ID", "id"),
        ]));

        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings[0].contains("NODE_ENV is not production"),
            "{warnings:?}"
        );
        assert!(
            warnings[1].contains("SESSION_SECRET is unset"),
            "{warnings:?}"
        );
        assert!(
            warnings[1].contains(super::DEFAULT_SESSION_SECRET),
            "the warning has to name the published value, or an operator cannot \
             tell which key is in use: {warnings:?}"
        );
    }

    /// A set secret drops that half and keeps the other. The relaxed CSP and
    /// the plaintext LiveKit URL do not depend on the secret, so a local run
    /// that sets one is still a local run.
    #[test]
    fn a_set_secret_leaves_only_the_mode_warning() {
        let warnings =
            super::relaxed_for_local_use(&values(&[("SESSION_SECRET", "a-real-secret")]));

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("NODE_ENV is not production"),
            "{warnings:?}"
        );
    }

    /// `is_production` matches the exact string, so a capitalized value is not
    /// production and the server really is running local defaults. Warning is
    /// the correct answer, and it is the only thing that catches the typo.
    #[test]
    fn a_misspelled_node_env_is_not_production_and_says_so() {
        let warnings = super::relaxed_for_local_use(&values(&[
            ("NODE_ENV", "Production"),
            ("SESSION_SECRET", "a-real-secret"),
        ]));

        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("NODE_ENV is not production"),
            "{warnings:?}"
        );
    }

    /// Production is the configured case and says nothing. A warning on every
    /// deployed start is a warning operators learn to scroll past, which is how
    /// the one above stops working.
    #[test]
    fn production_warns_about_nothing() {
        assert!(super::relaxed_for_local_use(&values(&[("NODE_ENV", "production")])).is_empty());
        assert!(
            super::relaxed_for_local_use(&values(&[
                ("NODE_ENV", "production"),
                ("SESSION_SECRET", "a-real-secret"),
            ]))
            .is_empty()
        );
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

        let config = super::select_provider(agent_config(pool.clone()), "interview-eu-a1b2c3d4")
            .expect("a configured provider should resolve");
        assert_eq!(config.livekit_url, "wss://eu.example");
        assert_eq!(config.livekit_api_secret, "eu-secret");
        assert_eq!(config.google_api_key, "eu-google");

        // No segment: what a single-provider deployment mints, and what a
        // hand-written INTERVIEW_ROOM_NAME usually looks like.
        let config = super::select_provider(agent_config(pool), "interview-a1b2c3d4").unwrap();
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
                super::select_provider(agent_config(pool.clone()), "interview-eu-a1b2c3d4")
            else {
                panic!("an unknown provider id should refuse");
            };
            assert!(error.contains("eu"), "{error}");
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

    /// The pair, not the id. `login_config` enables OAuth only when it has
    /// both, so an id with no secret verifies exactly as many people as no id
    /// at all, and would have started a server that refused every interview
    /// while looking configured.
    #[test]
    fn a_service_account_that_cannot_sign_stops_the_server() {
        // Not at the first delivery, which is an hour into somebody's
        // interview, and not as a warning that leaves the server running with
        // no way to hand a recording over.
        let mut recording = codetrial::config::RecordingConfig {
            livekit: None,
            gcs_bucket: "codetrial-staging".to_string(),
            gcs_prefix: "codetrial".to_string(),
            drive_id: "0AKfixtureDriveId".to_string(),
            service_account_json: r#"{"client_email":"a@b.iam.gserviceaccount.com","private_key":"-----BEGIN PRIVATE KEY-----
bm90IGEga2V5
-----END PRIVATE KEY-----
"}"#.to_string(),
            max_minutes: 45,
            bitrate: 2000,
            kill_switch: false,
            template_base_url: "https://recording.example".to_string(),
        };
        let error = super::recording_can_deliver(Some(&recording)).unwrap_err();
        assert!(
            error.contains("CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON"),
            "{error}"
        );

        // And a deployment that records nothing needs no credential at all.
        assert!(super::recording_can_deliver(None).is_ok());
        recording.service_account_json = String::new();
        assert!(super::recording_can_deliver(Some(&recording)).is_err());
    }

    #[test]
    fn recording_needs_both_oauth_credentials_or_it_refuses_to_start() {
        assert!(super::recording_needs_verified_identity(false, false).is_ok());
        assert!(super::recording_needs_verified_identity(true, true).is_ok());

        let error = super::recording_needs_verified_identity(true, false).unwrap_err();
        assert!(error.contains("GITHUB_CLIENT_ID"));
        assert!(error.contains("GITHUB_CLIENT_SECRET"));
    }
}

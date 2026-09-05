use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use codetrial::config::{
    AgentConfig, DEFAULT_DURATION_MIN, DEFAULT_ROOM_PREFIX, DEFAULT_WEB_ADDR, DEFAULT_WEB_DIR,
};
use codetrial::web::{RoomDispatcher, WebServerConfig};

/// One installation keeps its config file and its account database together in
/// `config/`, under whichever root it was installed as: a checkout, or the
/// folder a released binary was unpacked into. Beside the executable is not
/// that root for a checkout -- the executable is in `target/`, which
/// `make clean` deletes -- so the two are named separately and joined per case.
const CONFIG_DIR: &str = "config";
const CONFIG_FILE_NAME: &str = "codetrial.env.local";
/// Tracked, so it exists in a checkout and in no release archive. That makes it
/// the one thing that tells a `config/` belonging to this project apart from a
/// directory of the same name that happens to sit where the binary was run.
const CONFIG_EXAMPLE_NAME: &str = "codetrial.env.example";
const DEFAULT_ACCOUNT_DB_PATH: &str = "codetrial.db";
const DEFAULT_SESSION_SECRET: &str = "codetrial-local-session";

#[derive(Debug, Default, Clone)]
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
        // Logged for `web` only: `run-livekit`/`check-gemini` are terminal-run
        // tooling.
        run_web(options).inspect_err(|error| log_web_error(error))
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

    // No console survives to show a usage line on double-click; default to
    // `web`.
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
    // Falls through: once `serve_setup` returns, the config exists and the rest
    // of this function is an ordinary launch, on the socket Setup served on.
    // Why that socket is carried here rather than rebound: `serve_setup`. Read
    // once, so the question and the page it opens answer from the same map.
    let before_config = environment_and_flags(&options);
    let handed_over = if is_cold_start(&options, &before_config) {
        Some(serve_setup(&before_config)?)
    } else {
        None
    };
    let values = load_values(&options)?;

    if let Some(refusal) = production_secret_refusal(&values) {
        return Err(refusal);
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
        db_path: Some(account_db_path(&values, &options)),
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

    // The address is not re-read for a cold start: `serve_setup` read it from
    // the same environment and flags, and the config file written since holds
    // credentials, not `CODETRIAL_WEB_ADDR`.
    let listener = match handed_over {
        Some(listener) => listener,
        None => bind_web_listener(&value_or(&values, "CODETRIAL_WEB_ADDR", DEFAULT_WEB_ADDR))?,
    };

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

    // Past every startup check, so whatever a previous run left in the log is
    // no longer true. Written on failure and removed on none, the file outlives
    // what it describes: someone fixes what it named, starts again, and the
    // next thing to go wrong sends them back to a message from days ago. This
    // is the only moment that can tell those apart, and the error text points
    // at nothing else.
    clear_web_error_log();

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
        &config_dir(options),
        &value_or(values, codetrial::config::PROVIDER_ORDER_KEY, ""),
    );
    Ok(pool)
}

/// The solo self-serve cold start: no `--config` given, none of the searched
/// paths holds a file, and the environment has not supplied the credentials
/// either. A named-but-missing `--config` is an operator's mistake, not this.
///
/// The search order comes from `primary_config_path` rather than being
/// written out again here. Spelled twice it was free to drift, and the drift
/// is silent in the worst direction: a Setup page in front of someone whose
/// config the rest of the process is about to read.
///
/// The environment counts because a file is not the only way to be configured.
/// A headless launch with `LIVEKIT_*` exported and no file used to exit naming
/// the file it wanted, which is a failure someone reading a log can act on;
/// asking it instead put a page nobody would open in front of a process that
/// then waited forever. Only the LiveKit keys, because those are what the web
/// side refuses to start without, and `GOOGLE_API_KEY` decides whether this
/// process also hosts interviewers rather than whether it can run.
fn is_cold_start(options: &CliOptions, values: &BTreeMap<String, String>) -> bool {
    options.config_path.is_none()
        && primary_config_path(options).is_err()
        && !environment_supplies_credentials(values)
}

/// Whether this launch is already configured without a file.
///
/// Its own function so it can be asserted without a filesystem: `is_cold_start`
/// reaches this only after the config search misses, and the search hits in any
/// checkout that has a config, which is every machine a developer runs the
/// suite on.
///
/// The LiveKit keys and not `GOOGLE_API_KEY`, because those three are what the
/// web side refuses to start without. The Google key decides whether this
/// process also hosts interviewers, which is a shape rather than a requirement.
fn environment_supplies_credentials(values: &BTreeMap<String, String>) -> bool {
    ["LIVEKIT_URL", "LIVEKIT_API_KEY", "LIVEKIT_API_SECRET"]
        .iter()
        .all(|key| nonempty(values, key).is_some())
}

/// Serves Setup until a submission writes `codetrial.env.local`, then returns
/// the still-bound listener so `run_web` continues on it.
///
/// Handed over rather than rebound because of Windows, the platform this
/// feature exists for: `bind` sets `SO_REUSEADDR` on Unix and deliberately not
/// there, so a rebind has to win against the `TIME_WAIT` left by the
/// connections this page just served, and `TIME_WAIT` outlasts any retry worth
/// writing. The loser has already written the config, so it has also spent the
/// cold start that would have brought Setup back.
fn serve_setup(values: &BTreeMap<String, String>) -> Result<std::net::TcpListener, String> {
    let listener = bind_web_listener(&value_or(values, "CODETRIAL_WEB_ADDR", DEFAULT_WEB_ADDR))?;

    // Fails closed on an address the socket cannot name, for the reason
    // `run_web` does: an impossible kernel answer must not be the way past a
    // security guard.
    let bound = listener
        .local_addr()
        .map_err(|error| format!("bound listener has no address to check: {error}"))?;
    if let Some(refusal) = public_setup_refusal(values, bound) {
        return Err(refusal);
    }

    // Everything `run_web` will refuse for that this already knows the answer
    // to. There is one such rule today; the rest of its checks need the pool
    // the submission has not supplied yet.
    if let Some(refusal) = production_secret_refusal(values) {
        return Err(refusal);
    }
    // The window stays open now, but still needs to say where to go.
    println!("codetrial: open http://{bound} in your browser to continue setup");

    // Taken before `axum::serve` consumes the listener: this descriptor stays
    // open, so the port stays bound whatever the server does with its own.
    let retained = listener
        .try_clone()
        .map_err(|error| format!("could not retain the Setup listener: {error}"))?;

    // Re-asserted rather than assumed: whether a duplicate carries the flag
    // `bind_web_listener` set is a per-platform answer, and `from_std` in
    // `run_web` needs it true on this descriptor.
    retained
        .set_nonblocking(true)
        .expect("retained web listener should become nonblocking");

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should start");
    runtime
        .block_on(async {
            let ready = Arc::new(tokio::sync::Notify::new());
            let listener = tokio::net::TcpListener::from_std(listener)?;
            let service = codetrial::web::setup_service(
                ready.clone(),
                is_production(values),
                bound.port(),
                cold_start_config_path(),
            );
            axum::serve(listener, service)
                .with_graceful_shutdown(async move { ready.notified().await })
                .await
        })
        .map_err(|error| format!("web server failed: {error}"))?;
    Ok(retained)
}

/// Why a production launch may not sign session cookies, or `None` where it
/// may. The built-in default is a published string, so in production it is not
/// a weak signing key, it is a known one.
///
/// A function rather than an inline check because `serve_setup` asks it too.
/// The verdict needs nothing from the submission, and a check that could have
/// been made before the page was served but is made after it has taken
/// credentials, written them and answered 200 is a launch that exits with the
/// config already on disk -- which is what stops the next launch being a cold
/// start, so the page never comes back to say why.
fn production_secret_refusal(values: &BTreeMap<String, String>) -> Option<String> {
    if is_production(values)
        && value_or(values, "SESSION_SECRET", DEFAULT_SESSION_SECRET) == DEFAULT_SESSION_SECRET
    {
        return Some(
            "SESSION_SECRET must be set when NODE_ENV=production; the built-in \
             default is a published value that would let anyone forge a session cookie"
                .to_string(),
        );
    }
    None
}

/// Why Setup may not serve on `bound`, or `None` where it may.
///
/// Setup writes LiveKit credentials to disk for whoever posts them and has
/// nothing to authenticate that person with: on a cold start there is no
/// config, no account database and no secret either side could have agreed on
/// beforehand. What confines it is the listener, so the listener is what gets
/// checked. Adding a password to the page would only be a second credential
/// the same person has to be told over the same channel, and a cold start has
/// no console to print it on.
///
/// The reachability test is `published_secret_refusal`'s, whose doc says why it
/// is written this way and why a declared proxy counts.
///
/// Each reason names the one thing its reader can change, because the remedy
/// for the two is not the same: an address is passed on the command line, a
/// declared proxy is a variable in the environment, and "bind loopback" is no
/// help to someone already on it.
fn public_setup_refusal(
    values: &BTreeMap<String, String>,
    bound: std::net::SocketAddr,
) -> Option<String> {
    let reason = match reachable_from_elsewhere(values, bound)? {
        Reachable::Address => {
            format!("it is bound to {bound}, which is not a loopback address")
        }
        Reachable::DeclaredProxy => "CODETRIAL_TRUSTED_PROXY_HOPS says something in front of \
             it forwards requests from elsewhere"
            .to_string(),
    };

    Some(format!(
        "refusing to serve the Setup page because {reason}: it takes LiveKit credentials \
         from anyone who can reach it, over plain HTTP, and has nothing to authenticate \
         them with. Serve it where only this machine can reach it, or write \
         codetrial.env.local with LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET \
         before starting."
    ))
}

/// Any `web` failure, not just a cold-start one. No attempt to tell a solo
/// user's config apart from an operator's: the error text already says
/// what's wrong. Best-effort: must not shadow the real error.
///
/// Beside the executable, where the config it just failed to read also
/// lives. A double-clicked binary has no console to leave the reason in and
/// no working directory anyone chose, so the folder it was unpacked into is
/// the one place its owner knows to look.
fn log_web_error(error: &str) {
    let _ = std::fs::write(web_error_log_path(), format!("{error}\n"));
}

/// Dropped once a launch is past everything that could have written one, so
/// the file's presence means the last `web` start failed rather than that one
/// ever did. Best-effort, like the write: a log that cannot be removed must
/// not stop a server that is otherwise ready.
fn clear_web_error_log() {
    let _ = std::fs::remove_file(web_error_log_path());
}

fn web_error_log_path() -> std::path::PathBuf {
    codetrial::exe_dir().join("codetrial-error.log")
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
        &config_dir(options),
        &provider_order,
    );
    Ok(config)
}

/// The environment and the flags, which in a cold start is everything
/// `load_values` has: the file it reads between them is the file that does not
/// exist yet.
///
/// Its own function so `run_web` does not name the process environment. Reading
/// a deployment key straight from it there is what
/// `binary_web_reads_deployment_keys_from_the_config_file` refuses, because a
/// key set in the config file was once silently a no-op.
fn environment_and_flags(options: &CliOptions) -> BTreeMap<String, String> {
    let mut values = std::env::vars().collect::<BTreeMap<_, _>>();
    apply_options(&mut values, options);
    values
}

fn load_values(options: &CliOptions) -> Result<BTreeMap<String, String>, String> {
    let mut values = std::env::vars().collect::<BTreeMap<_, _>>();
    let path = primary_config_path(options)?;
    for (key, value) in codetrial::config::read_config_file(&path)? {
        values.insert(key, value);
    }
    apply_options(&mut values, options);
    Ok(values)
}

/// The flags, over whatever the environment and the config file said.
///
/// Split out because a cold start has to build the same map without the file
/// it does not have yet: `serve_setup` decides where to bind and what a
/// submission must survive, and both answers have to be the ones `run_web`
/// will reach a moment later. Spelled twice, this precedence is free to drift.
fn apply_options(values: &mut BTreeMap<String, String>, options: &CliOptions) {
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
    //
    // Within each of those two roots, `config/` before the bare filename. The
    // bare ones are where a release used to keep its config and where an
    // operator may still keep one; they are searched, not written.
    for path in [
        PathBuf::from(CONFIG_DIR).join(CONFIG_FILE_NAME),
        PathBuf::from(CONFIG_FILE_NAME),
        codetrial::exe_dir().join(CONFIG_DIR).join(CONFIG_FILE_NAME),
        codetrial::exe_dir().join(CONFIG_FILE_NAME),
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
        "required configuration file is missing: ./{CONFIG_DIR}/{CONFIG_FILE_NAME}, \
         ./{CONFIG_FILE_NAME}, or {CONFIG_DIR}/{CONFIG_FILE_NAME} beside the \
         executable; write one with LIVEKIT_URL, LIVEKIT_API_KEY and \
         LIVEKIT_API_SECRET ({CONFIG_DIR}/{CONFIG_EXAMPLE_NAME} lists the \
         optional keys in a checkout)"
    ))
}

/// The directory of the config file this launch actually read, which is where
/// everything else belonging to the installation lives: the provider files it
/// discovers, and the account database it writes.
///
/// So `--config /etc/codetrial/prod.env` discovers
/// `/etc/codetrial/codetrial.env.<id>` rather than whatever `config/` happens
/// to sit in the current directory. Both halves of a deployment are launched
/// with the same `--config`, and that is what makes them agree on the pool.
///
/// The database is written here rather than read, which is the one asymmetry:
/// a `--config` pointing somewhere a process may not write is a startup
/// failure that names the directory, and `CODETRIAL_DB_PATH` is the way out.
fn config_dir(options: &CliOptions) -> PathBuf {
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
        .unwrap_or_else(|| PathBuf::from(CONFIG_DIR).join(CONFIG_FILE_NAME));
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
/// How a listener on `bound` can be reached from somewhere other than this
/// machine, or `None` where it cannot.
///
/// Shared, because two guards ask it and a third answer added to one of them
/// would drift silently -- in the direction that leaves the page taking
/// credentials open. What each guard says about it is not shared: the remedies
/// differ, an address is passed on the command line and a declared proxy is a
/// variable in the environment.
enum Reachable {
    /// The bind address is not loopback.
    Address,
    /// `CODETRIAL_TRUSTED_PROXY_HOPS` is the operator saying requests arrive
    /// through something in front, which is the same admission: a server on
    /// loopback behind nginx or a tunnel is as public as one on `0.0.0.0`.
    DeclaredProxy,
}

fn reachable_from_elsewhere(
    values: &BTreeMap<String, String>,
    bound: std::net::SocketAddr,
) -> Option<Reachable> {
    if !bound.ip().to_canonical().is_loopback() {
        Some(Reachable::Address)
    } else if trusted_proxy_hops(values) > 0 {
        Some(Reachable::DeclaredProxy)
    } else {
        None
    }
}

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

    let reason = match reachable_from_elsewhere(values, bound)? {
        Reachable::Address => format!("bind {bound}"),
        Reachable::DeclaredProxy => "serve from behind a declared proxy".to_string(),
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
///
/// Beside the config file rather than in the working directory: the database
/// has to be the same file on the next launch, and the directory a process was
/// started from is not. It used to be a bare filename, which meant a launch
/// from a shortcut or from another terminal found the config it wrote and then
/// opened an empty database next to wherever it had been started.
fn account_db_path(values: &BTreeMap<String, String>, options: &CliOptions) -> PathBuf {
    match nonempty(values, "CODETRIAL_DB_PATH") {
        Some(path) => PathBuf::from(path),
        None => config_dir(options).join(DEFAULT_ACCOUNT_DB_PATH),
    }
}

/// Where a cold start writes the config it just took, which is the same
/// `config/` the search above looks in first for the root this is installed as.
///
/// A checkout is told apart by the template it ships, not by `config/` merely
/// existing: a released binary run from a directory that happens to hold one
/// would otherwise write the credentials there and lose them the moment it was
/// next started from somewhere else.
fn cold_start_config_path() -> PathBuf {
    let in_checkout = PathBuf::from(CONFIG_DIR);
    if in_checkout.join(CONFIG_EXAMPLE_NAME).is_file() {
        return in_checkout.join(CONFIG_FILE_NAME);
    }
    codetrial::exe_dir().join(CONFIG_DIR).join(CONFIG_FILE_NAME)
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

    /// The precedence two launches depend on: a cold start builds this map
    /// without a config file and has to reach the same address and the same
    /// verdict `run_web` will reach with one. A flag overrides what was in the
    /// environment; no flag leaves it alone.
    #[test]
    fn the_flags_override_the_environment_and_only_where_given() {
        let mut values = values(&[
            ("CODETRIAL_WEB_ADDR", "127.0.0.1:1"),
            ("CODETRIAL_WEB_DIR", "env-dir"),
            ("NODE_ENV", "production"),
        ]);
        super::apply_options(
            &mut values,
            &super::CliOptions {
                web_addr: Some("127.0.0.1:2".to_string()),
                room_prefix: Some("flag".to_string()),
                duration_min: Some(45),
                ..super::CliOptions::default()
            },
        );

        assert_eq!(values["CODETRIAL_WEB_ADDR"], "127.0.0.1:2");
        assert_eq!(values["CODETRIAL_ROOM_PREFIX"], "flag");
        assert_eq!(values["CODETRIAL_DURATION_MIN"], "45");
        assert_eq!(
            values["CODETRIAL_WEB_DIR"], "env-dir",
            "a flag that was not passed must not erase the environment"
        );
        assert_eq!(
            values["NODE_ENV"], "production",
            "and must not touch the rest"
        );
    }

    /// Setup is confined to the same interface the published session secret is,
    /// and for a sharper reason: this listener hands `codetrial.env.local` to
    /// whoever posts to it. Unlike `published_secret_refusal` there is no value
    /// an operator can set to lift it, so every address is tested against one
    /// empty environment.
    #[test]
    fn the_setup_page_is_confined_to_loopback() {
        for local in [
            "127.0.0.1:3000",
            "127.0.0.53:3000",
            "[::1]:3000",
            "[::ffff:127.0.0.1]:3000",
        ] {
            assert_eq!(
                super::public_setup_refusal(&values(&[]), addr(local)),
                None,
                "{local} reaches no further than this machine"
            );
        }

        for public in ["0.0.0.0:3000", "[::]:3000", "192.168.1.10:3000"] {
            let refusal = super::public_setup_refusal(&values(&[]), addr(public))
                .unwrap_or_else(|| panic!("{public} must be refused"));
            assert!(refusal.contains(public), "{refusal}");
            assert!(refusal.contains("codetrial.env.local"), "{refusal}");
        }
    }

    /// The same admission that makes a loopback bind public for the session
    /// secret makes it public for Setup: a tunnel or an nginx in front is how
    /// the internet reaches 127.0.0.1.
    #[test]
    fn a_declared_proxy_closes_the_setup_page_too() {
        let refusal = super::public_setup_refusal(
            &values(&[("CODETRIAL_TRUSTED_PROXY_HOPS", "1")]),
            addr("127.0.0.1:3000"),
        )
        .expect("loopback behind a declared proxy must be refused");

        // The variable by name: it is the only thing the reader of this message
        // can change, and the address they are on is already loopback.
        assert!(
            refusal.contains("CODETRIAL_TRUSTED_PROXY_HOPS"),
            "{refusal}"
        );

        assert_eq!(
            super::public_setup_refusal(
                &values(&[("CODETRIAL_TRUSTED_PROXY_HOPS", "0")]),
                addr("127.0.0.1:3000")
            ),
            None,
            "no declared proxy is the solo cold start this page exists for"
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

    /// A `--config` that names nothing is an operator's mistake, which
    /// `primary_config_path` reports by name. Reading it as a cold start
    /// instead would answer a wrong path with a Setup page and write a second
    /// config beside the one they meant. Both halves are asserted here because
    /// the missing file is the only case where the two conditions disagree,
    /// and it is a unit test rather than a spawned binary because a launch
    /// that wrongly believes it is a cold start serves Setup and waits there.
    #[test]
    fn a_named_config_is_never_a_cold_start() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let named = |path: std::path::PathBuf| super::CliOptions {
            config_path: Some(path.display().to_string()),
            ..super::CliOptions::default()
        };

        let empty = values(&[]);
        assert!(
            !super::is_cold_start(&named(root.join("Cargo.toml")), &empty),
            "a config file that exists was named, so there is nothing to set up"
        );
        assert!(
            !super::is_cold_start(&named(root.join("no-such-config.env")), &empty),
            "a config file that is missing was still named, and a name is an \
             instruction to read that file rather than an invitation to ask"
        );
    }

    /// Exported credentials are a configured launch, so there is nothing to ask
    /// for. Asking anyway put a page nobody would open in front of a headless
    /// start that then waited forever, where naming the missing file and
    /// exiting was something a log could carry.
    #[test]
    fn exported_credentials_are_a_configured_launch() {
        assert!(super::environment_supplies_credentials(&values(&[
            ("LIVEKIT_URL", "wss://example"),
            ("LIVEKIT_API_KEY", "key"),
            ("LIVEKIT_API_SECRET", "secret"),
        ])));

        // One short of the set the web side refuses to start without, which is
        // still a launch that has to be asked.
        assert!(!super::environment_supplies_credentials(&values(&[
            ("LIVEKIT_URL", "wss://example"),
            ("LIVEKIT_API_KEY", "key"),
        ])));

        // Present and blank is missing, the way `nonempty` reads it everywhere
        // else, and the way `read_config_file` would have written it back.
        assert!(!super::environment_supplies_credentials(&values(&[
            ("LIVEKIT_URL", "wss://example"),
            ("LIVEKIT_API_KEY", "key"),
            ("LIVEKIT_API_SECRET", "   "),
        ])));

        // The optional one is not part of the question.
        assert!(super::environment_supplies_credentials(&values(&[
            ("LIVEKIT_URL", "wss://example"),
            ("LIVEKIT_API_KEY", "key"),
            ("LIVEKIT_API_SECRET", "secret"),
            ("GOOGLE_API_KEY", ""),
        ])));
    }

    #[test]
    fn bare_config_paths_discover_providers_from_the_current_directory() {
        let options = super::CliOptions {
            config_path: Some("prod.env".to_string()),
            ..Default::default()
        };
        assert_eq!(super::config_dir(&options), std::path::PathBuf::from("."));
    }

    /// The database is the config directory's, so a named config carries it
    /// along rather than leaving it wherever the process was started.
    #[test]
    fn the_account_database_sits_beside_the_config_that_was_named() {
        let options = super::CliOptions {
            config_path: Some("/etc/codetrial/prod.env".to_string()),
            ..Default::default()
        };
        assert_eq!(
            super::account_db_path(&values(&[]), &options),
            std::path::PathBuf::from("/etc/codetrial/codetrial.db")
        );
        assert_eq!(
            super::account_db_path(
                &values(&[("CODETRIAL_DB_PATH", "/srv/accounts.db")]),
                &options
            ),
            std::path::PathBuf::from("/srv/accounts.db")
        );
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

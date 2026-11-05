use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use codetrial::config::{AgentConfig, DEFAULT_WEB_ADDR, is_production};
use codetrial::launch::{
    CliOptions, LaunchedBy, Reachable, cold_start_config_path, config_dir,
    data_dir_config_from_environment, environment_and_flags, extend_pool, finish, load_values,
    local_agent_config, nonempty, prepare, primary_config_path, production_secret_refusal,
    reachable_from_elsewhere, serve, value_or,
};
#[cfg(test)]
use codetrial::launch::{
    DEFAULT_SESSION_SECRET, SESSION_SECRET_FILE, account_db_path, allowed_hosts, apply_options,
    data_dir_session_secret, environment, guest_mode_enabled, guest_mode_recording_conflict,
    published_secret_refusal, recording_can_deliver, recording_needs_verified_identity,
    relaxed_for_local_use, task_mode_allowed, task_mode_enabled, task_mode_refusal,
    web_server_config,
};

/// Every mode this binary has: its name, how many positionals it takes counting
/// the mode word, how it is called, and what runs it. One table and not one
/// list
/// per question, because a mode that exists in the dispatch but not in the
/// arity
/// check is a mode that silently ignores its extra arguments.
const MODES: [(&str, usize, &str, ModeFn); 3] = [
    ("web", 1, "codetrial web [OPTIONS]", |_, options| {
        // Logged for `web` only: `run-livekit`/`check-gemini` are terminal-run
        // tooling. The path is taken before `run_web` owns the options.
        let log = web_error_log_path(&options);
        run_web(options).inspect_err(|error| log_web_error(&log, error))
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
    codetrial::install_crypto_provider();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    std::process::exit(run_agent_command(&args));
}

fn build_version() -> String {
    format!(
        "codetrial {} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("CODETRIAL_BUILD_COMMIT")
    )
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
    if args
        .iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--version" || arg == "-V")
    {
        println!("{}", build_version());
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

    if mode == "web" {
        println!("{}", build_version());
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

        // The two options that take no value: they must not read the next word
        // as one, which would swallow a positional.
        if arg == "--task-mode" {
            options.task_mode = true;
            continue;
        }
        if arg == "--no-update-check" {
            options.no_update_check = true;
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
            "--data-dir" => options.data_dir = Some(PathBuf::from(value)),
            "--import-config" => options.import_config = Some(PathBuf::from(value)),
            "--web-addr" => options.web_addr = Some(value),
            "--allowed-hosts" if value.trim().is_empty() => {
                return Err("--allowed-hosts requires a Host".to_string());
            }
            "--allowed-hosts" => options.allowed_hosts = Some(value),
            "--web-dir" => options.web_dir = Some(value),
            "--task-mode" => return Err("--task-mode takes no value".to_string()),
            "--no-update-check" => return Err("--no-update-check takes no value".to_string()),
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

    // Two answers to where the config file is. `--data-dir` exists so that a
    // launcher can say everything lives in one folder, and a `--config` beside
    // it would put the file somewhere else while the database and the log
    // stayed behind.
    if options.config_path.is_some() && options.data_dir.is_some() {
        return Err(
            "--config and --data-dir cannot be combined: --data-dir already names the \
             folder the config file is read from"
                .to_string(),
        );
    }

    // It is offered on a data folder's first Setup page, and without a data
    // folder the CLI searches for the file itself.
    if options.import_config.is_some() && options.data_dir.is_none() {
        return Err(
            "--import-config needs --data-dir: it is offered on a data folder's Setup page"
                .to_string(),
        );
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
        "\nOptions:\n  --config PATH        Use PATH instead of config/codetrial.env.local\n  --data-dir DIR       Keep the config, accounts and error log in DIR only\n  --import-config PATH Offer the Setup keys in PATH on DIR's Setup page\n  --web-addr ADDR      Listen on ADDR\n  --allowed-hosts LIST Answer only these comma-separated Host values\n  --web-dir PATH       Serve files from PATH\n  --room-prefix PREFIX Prefix generated room names\n  --duration-min MIN   Set the interview duration\n  --task-mode          Serve learner task mode, off unless given (web only)\n  --no-update-check    Never ask GitHub for a newer release (web only)\n  -h, --help           Show this help\n  -V, --version        Show the package version and build commit\n\nEvery option also takes `--flag=value`, which is the only way to pass a value\nthat starts with a dash. `--` ends the options."
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
    let launched_by = if codetrial::exit_when_stdin_closes() {
        LaunchedBy::Parent
    } else {
        LaunchedBy::Operator
    };
    data_dir_config_from_environment(&options, is_production(&before_config))?;
    let handed_over = if is_cold_start(&options, &before_config) {
        Some(serve_setup(&before_config, &options)?)
    } else {
        None
    };
    let values = load_values(&options)?;

    let prepared = prepare(&values, &options)?;
    let max_concurrent = prepared.max_concurrent;
    let config = prepared.config;

    // The address is not re-read for a cold start: `serve_setup` read it from
    // the same environment and flags, and the config file written since holds
    // credentials, not `CODETRIAL_WEB_ADDR`.
    let listener = match handed_over {
        Some(listener) => listener,
        None => bind_web_listener(&value_or(&values, "CODETRIAL_WEB_ADDR", DEFAULT_WEB_ADDR))?,
    };

    // The other half of the `SESSION_SECRET` guard in `launch::prepare`, and
    // the half that does not depend on the operator having said anything.
    //
    // After the bind rather than before it, because the address may be a
    // hostname: `CODETRIAL_WEB_ADDR=interviews.example:3000` is resolved by
    // `ToSocketAddrs` and only the socket knows what it landed on. A refusal
    // here therefore comes after the bind, with the consequence that follows:
    // the socket exists and its backlog fills, nothing accepts from it, and the
    // process exits non-zero. (`launch::prepare`, with `initialize_accounts`,
    // runs before the bind and has no such cost.)
    //
    // Fails closed when the socket cannot name itself, which a just-bound
    // listener does not do. Skipping the check on an unreadable address would
    // make an impossible kernel answer the one way past a security guard, and
    // refusing to start is the cheaper half of that trade.
    let bound = listener
        .local_addr()
        .map_err(|error| format!("bound listener has no address to check: {error}"))?;
    let (config, setup) = finish(config, &values, &options, bound, launched_by)?;

    // Past every startup check, so whatever a previous run left in the log is
    // no longer true. Written on failure and removed on none, the file outlives
    // what it describes: someone fixes what it named, starts again, and the
    // next thing to go wrong sends them back to a message from days ago. This
    // is the only moment that can tell those apart, and the error text points
    // at nothing else.
    clear_web_error_log(&options);

    let agent_config = local_agent_config(values)?;

    serve(
        listener,
        config,
        setup,
        agent_config,
        max_concurrent,
        std::future::pending(),
        None,
    )
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
fn serve_setup(
    values: &BTreeMap<String, String>,
    options: &CliOptions,
) -> Result<std::net::TcpListener, String> {
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
                Arc::clone(&ready),
                is_production(values),
                bound.port(),
                cold_start_config_path(options),
                options.import_config.clone(),
            );
            axum::serve(listener, service)
                .with_graceful_shutdown(async move { ready.notified().await })
                .await
        })
        .map_err(|error| format!("web server failed: {error}"))?;
    Ok(retained)
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
///
/// Headed by the same build line the console shows first. This file is what
/// arrives when that owner reports the failure, and a report that cannot name
/// its build cannot say whether a fix is already in it.
fn log_web_error(path: &std::path::Path, error: &str) {
    // A `--data-dir` that does not exist yet is the usual first failure, and
    // the folder it names is where its owner will look for the reason.
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, format!("{}\n{error}\n", build_version()));
}

/// Dropped once a launch is past everything that could have written one, so
/// the file's presence means the last `web` start failed rather than that one
/// ever did. Best-effort, like the write: a log that cannot be removed must
/// not stop a server that is otherwise ready.
fn clear_web_error_log(options: &CliOptions) {
    let _ = std::fs::remove_file(web_error_log_path(options));
}

/// In the data directory when there is one: an installed app's executable
/// folder is read-only, or inside an application bundle nobody opens.
fn web_error_log_path(options: &CliOptions) -> std::path::PathBuf {
    match &options.data_dir {
        Some(dir) => dir.join("codetrial-error.log"),
        None => codetrial::exe_dir().join("codetrial-error.log"),
    }
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
            codetrial::gemini::GeminiKeys::from_config(&config).redact(&error.to_string())
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
        let keys = codetrial::gemini::GeminiKeys::from_config(&config);
        let session = codetrial::gemini::check_live_session(&keys, &boot).await?;
        session.close().await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(boot)
    });
    let boot = result.map_err(|error| {
        codetrial::gemini::GeminiKeys::from_config(&config).redact(&error.to_string())
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
fn select_provider(config: AgentConfig, room_name: &str) -> Result<AgentConfig, String> {
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
    Ok(codetrial::dispatch::agent_config_for(&config, provider))
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

#[cfg(test)]
// `bin.rs`, not `main.rs`: cargo reads `tests/<dir>/main.rs` as an
// integration-test target of its own, which would compile this file a second
// time as a crate root, where every `super::` in it is an error.
#[path = "../tests/unit/bin.rs"]
mod tests;

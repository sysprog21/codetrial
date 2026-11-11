//! How a launch turns its flags, environment and config file into a server.
//!
//! Lives in the library, not the binary, so a test fixture builds the server
//! from the same constructors `codetrial web` does instead of a copy that only
//! resembles them.

use std::collections::BTreeMap;
use std::path::PathBuf;

mod peer;

pub use peer::{PeerServices, peer_services};

use crate::config::{DEFAULT_DURATION_MIN, DEFAULT_ROOM_PREFIX, DEFAULT_WEB_DIR};
use crate::web::WebServerConfig;

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

pub const DEFAULT_SESSION_SECRET: &str = "codetrial-local-session";

/// Where a `--data-dir` install keeps the key its session cookies are signed
/// with, when its config file does not set `SESSION_SECRET`.
pub const SESSION_SECRET_FILE: &str = "session-secret";

#[derive(Debug, Default, Clone)]
pub struct CliOptions {
    pub config_path: Option<String>,
    pub data_dir: Option<PathBuf>,
    pub import_config: Option<PathBuf>,
    pub web_addr: Option<String>,
    pub web_dir: Option<String>,
    pub allowed_hosts: Option<String>,
    pub room_prefix: Option<String>,
    pub duration_min: Option<u32>,
    pub task_mode: bool,
    pub no_update_check: bool,
}

/// Validates operator settings before a listener or background task starts.
pub fn web_server_config(
    values: &std::collections::BTreeMap<String, String>,
    pool: crate::config::ProviderPool,
    production: bool,
    options: &CliOptions,
) -> Result<WebServerConfig, String> {
    // After the pool is complete, so the "is this project in the pool" check
    // sees the pool the server will actually serve, and before the listener
    // does any work: a half-configured recording block is a startup failure,
    // never a surprise at the moment a candidate's interview ends.
    let recording =
        crate::config::load_recording(values, &pool, interview_duration_min(values), production)
            .map_err(|error| error.to_string())?;
    recording_needs_verified_identity(
        recording.is_some(),
        nonempty(values, "GITHUB_CLIENT_ID").is_some()
            && nonempty(values, "GITHUB_CLIENT_SECRET").is_some(),
    )?;
    recording_can_deliver(recording.as_ref())?;
    let (peer, peer_observer) = peer::peer_services(
        values,
        nonempty(values, "GITHUB_CLIENT_ID").is_some()
            && nonempty(values, "GITHUB_CLIENT_SECRET").is_some(),
    )?
    .unzip();

    let config = WebServerConfig {
        tasks: None,
        task_mode_requested: options.task_mode,
        web_dir: match nonempty(values, "CODETRIAL_WEB_DIR") {
            Some(dir) => PathBuf::from(dir),

            // Under the data directory rather than the working one, which a
            // launcher does not choose: a `web/` that happens to sit where the
            // app was started would otherwise replace the embedded pages.
            None => match &options.data_dir {
                Some(dir) => dir.join(DEFAULT_WEB_DIR),
                None => PathBuf::from(DEFAULT_WEB_DIR),
            },
        },
        github_client_id: nonempty(values, "GITHUB_CLIENT_ID"),
        github_client_secret: nonempty(values, "GITHUB_CLIENT_SECRET"),
        session_secret: Some(value_or(values, "SESSION_SECRET", DEFAULT_SESSION_SECRET)),
        db_path: Some(account_db_path(values, options)),
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: value_or(values, "CODETRIAL_ROOM_PREFIX", DEFAULT_ROOM_PREFIX),
        fixed_room_name: nonempty(values, "INTERVIEW_ROOM_NAME"),
        production,
        trusted_proxy_hops: trusted_proxy_hops(values)?,
        allowed_hosts: allowed_hosts(values)?,
        compiler_explorer_enabled: compiler_explorer_enabled(values),
        guest_mode_enabled: guest_mode_enabled(values),
        recording,
        peer_observer,
        peer,
        pool,
        probe_provider_quota: true,

        // Not a function of its own: off macOS this is always `None`, so no
        // test on the Linux CI could tell a replaced body from this one.
        update_check_commit: crate::config::update_check_commit(
            values.get("CODETRIAL_UPDATE_CHECK").map(String::as_str),
            env!("CODETRIAL_RELEASE_COMMIT"),
            cfg!(target_os = "macos"),
        ),
        provision_rooms: true,
    };
    if let Some(observer) = &config.peer_observer {
        observer.validate_audio_storage(&config.web_dir)?;
    }
    Ok(config)
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
) -> Result<crate::config::ProviderPool, String> {
    let mut pool = crate::config::ProviderPool::default();
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
    crate::config::validate_livekit_url(&url, production)?;
    pool.providers.push(crate::config::Provider {
        id: crate::config::PRIMARY_PROVIDER_ID.to_string(),
        url,
        api_key,
        api_secret,
        google_api_keys: crate::config::google_api_keys(values)?,
    });
    extend_pool(
        &mut pool,
        production,
        &config_dir(options),
        &value_or(values, crate::config::PROVIDER_ORDER_KEY, ""),
    );
    Ok(pool)
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
pub fn production_secret_refusal(values: &BTreeMap<String, String>) -> Option<String> {
    if crate::config::is_production(values)
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

/// The environment and the flags, which in a cold start is everything
/// `load_values` has: the file it reads between them is the file that does not
/// exist yet.
///
/// Its own function so `run_web` does not name the process environment. Reading
/// a deployment key straight from it there is what
/// `binary_web_reads_deployment_keys_from_the_config_file` refuses, because a
/// key set in the config file was once silently a no-op.
pub fn environment_and_flags(options: &CliOptions) -> BTreeMap<String, String> {
    let mut values = environment(options);
    apply_options(&mut values, options);
    values
}

/// A first `--data-dir` launch started where the credentials Setup asks for
/// are already exported writes them into the folder's config, as Setup would
/// have, instead of asking for them again; every later launch then finds the
/// file, including one started from a desktop that exports nothing.
///
/// The one exception to `environment` ignoring the process environment, and a
/// narrow one: only Setup's own keys, only while the folder has no config, and
/// written down rather than read live, so the file stays the whole of the
/// install's configuration.
pub fn data_dir_config_from_environment(
    options: &CliOptions,
    production: bool,
) -> Result<(), String> {
    let Some(dir) = &options.data_dir else {
        return Ok(());
    };
    let path = dir.join(CONFIG_FILE_NAME);
    if path.exists() {
        return Ok(());
    }
    let exported = std::env::vars().collect();
    let Some(contents) =
        crate::web::config_from_values(&exported, production).map_err(|error| {
            format!("the exported Setup keys cannot be written to a config: {error}")
        })?
    else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)
        .and_then(|()| crate::create_owner_only(&path, contents.as_bytes()))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    println!(
        "codetrial: wrote {} from the exported LiveKit and Google keys",
        path.display()
    );
    Ok(())
}

/// The process environment, or nothing for a `--data-dir` launch.
///
/// A launcher that names the folder is saying the install's configuration is
/// in it, and an environment it was started from is not part of that: a shell
/// that exported `CODETRIAL_DB_PATH` or `NODE_ENV` for a CLI deployment would
/// otherwise move this install's reports, or change its rules, depending on
/// how it was started. Decided here, once, rather than by every launcher
/// keeping its own list of the keys this program reads.
pub fn environment(options: &CliOptions) -> BTreeMap<String, String> {
    if options.data_dir.is_some() {
        return BTreeMap::new();
    }
    std::env::vars().collect()
}

pub fn load_values(options: &CliOptions) -> Result<BTreeMap<String, String>, String> {
    let mut values = environment(options);
    let path = primary_config_path(options)?;
    crate::config::apply_config_file(&mut values, crate::config::read_config_file(&path)?);
    apply_options(&mut values, options);
    if let Some(dir) = &options.data_dir
        && nonempty(&values, "SESSION_SECRET").is_none()
    {
        let secret = data_dir_session_secret(&dir.join(SESSION_SECRET_FILE))?;
        values.insert("SESSION_SECRET".to_string(), secret);
    }
    Ok(values)
}

/// The key a `--data-dir` install signs session cookies with, made on its first
/// launch and reused, so a relaunch keeps everyone signed in.
///
/// Not the built-in default: that one is published, and any program on the
/// machine can reach a loopback port to present a cookie signed with it.
/// Owner-only on Unix for the same reason. A file that is not a key this wrote
/// is refused rather than replaced, because replacing it signs everyone out
/// and hides whatever damaged it.
pub fn data_dir_session_secret(path: &std::path::Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let secret = text.trim();
            if secret.len() >= 32
                && secret
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            {
                Ok(secret.to_string())
            } else {
                Err(format!(
                    "{} is not a session key this program wrote. Move it aside to make a new \
                     one; everyone will be signed out.",
                    path.display()
                ))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let secret = crate::accounts::random_token(32)
                .map_err(|error| format!("cannot generate a session key: {error}"))?;
            crate::create_owner_only(path, secret.as_bytes())
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
            Ok(secret)
        }
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

/// The flags, over whatever the environment and the config file said.
///
/// Split out because a cold start has to build the same map without the file
/// it does not have yet: `serve_setup` decides where to bind and what a
/// submission must survive, and both answers have to be the ones `run_web`
/// will reach a moment later. Spelled twice, this precedence is free to drift.
pub fn apply_options(values: &mut BTreeMap<String, String>, options: &CliOptions) {
    if let Some(value) = &options.web_addr {
        values.insert("CODETRIAL_WEB_ADDR".to_string(), value.clone());
    }
    if let Some(value) = &options.web_dir {
        values.insert("CODETRIAL_WEB_DIR".to_string(), value.clone());
    }
    if let Some(value) = &options.allowed_hosts {
        values.insert("CODETRIAL_ALLOWED_HOSTS".to_string(), value.clone());
    }
    if let Some(value) = &options.room_prefix {
        values.insert("CODETRIAL_ROOM_PREFIX".to_string(), value.clone());
    }
    if let Some(value) = options.duration_min {
        values.insert("CODETRIAL_DURATION_MIN".to_string(), value.to_string());
    }

    // Over the config file as well: the desktop app is published as its own
    // release, so the one the check reads never names its build.
    if options.no_update_check {
        values.insert("CODETRIAL_UPDATE_CHECK".to_string(), "false".to_string());
    }
}

pub fn primary_config_path(options: &CliOptions) -> Result<PathBuf, String> {
    // The one place searched, and not one place among several: the working
    // directory and the executable's folder are whatever the launcher left them
    // as, so a config found there would belong to some other install.
    if let Some(dir) = &options.data_dir {
        let path = dir.join(CONFIG_FILE_NAME);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "required configuration file is missing: {}; write one with LIVEKIT_URL, \
             LIVEKIT_API_KEY and LIVEKIT_API_SECRET",
            path.display()
        ));
    }
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
        crate::exe_dir().join(CONFIG_DIR).join(CONFIG_FILE_NAME),
        crate::exe_dir().join(CONFIG_FILE_NAME),
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
pub fn config_dir(options: &CliOptions) -> PathBuf {
    if let Some(dir) = &options.data_dir {
        return dir.clone();
    }

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
pub fn extend_pool(
    pool: &mut crate::config::ProviderPool,
    production: bool,
    config_dir: &std::path::Path,
    order: &str,
) {
    let (providers, warnings) = crate::config::discover_providers(config_dir, production);
    for warning in warnings {
        eprintln!("{warning}");
    }
    pool.providers.extend(providers);

    // Outside the `discover` guard: an order naming the environment provider is
    // still an order, and a deployment with discovery off should not silently
    // ignore the one it was given.
    for warning in crate::config::order_providers(&mut pool.providers, order) {
        eprintln!("{warning}");
    }

    for line in pool_report(&pool.providers) {
        eprintln!("{line}");
    }
}

/// What startup says about the pool, in rotation order, and nothing for an
/// empty one. Every provider question asked of a running server so far has been
/// answerable only by reading the config directory and the argv back to itself
/// and guessing which won.
fn pool_report(providers: &[crate::config::Provider]) -> Vec<String> {
    if providers.is_empty() {
        return Vec::new();
    }
    let (summary, warnings) = crate::config::pool_summary(providers);
    std::iter::once(summary)
        .chain(
            warnings
                .into_iter()
                .map(|warning| format!("WARNING: {warning}")),
        )
        .collect()
}

/// Refuses to start on a database that will not migrate, before anything is
/// served. The listener is already bound by this point, so the socket exists
/// and its backlog fills, but nothing accepts from it and the process exits
/// non-zero instead. The router runs the same migration again on the connection
/// it keeps, where the only thing it can do about a failure is answer 503.
pub fn initialize_accounts(config: &WebServerConfig) -> Result<(), String> {
    let Some(login) = crate::web::login_config(config) else {
        return Ok(());
    };

    // The router opens the long-lived connection and performs the startup sweep
    // on it; this first pass makes migration failure fatal before the server
    // starts accepting.
    crate::web::initialize_account_database(&login.db_path)
        .map_err(|error| format!("account database {}: {error}", login.db_path.display()))?;
    Ok(())
}

/// The dispatcher cap, with anything unusable about it said out loud.
///
/// One helper for both modes: a warning printed by web and swallowed by serve
/// would be the same misconfiguration reported twice differently.
fn read_max_concurrent(values: &BTreeMap<String, String>) -> usize {
    let mut warnings = Vec::new();
    let max_concurrent = crate::config::max_concurrent_interviews(values, &mut warnings);
    for warning in &warnings {
        eprintln!("{warning}");
    }
    max_concurrent
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
pub fn recording_can_deliver(
    recording: Option<&crate::config::RecordingConfig>,
) -> Result<(), String> {
    let Some(recording) = recording else {
        return Ok(());
    };
    crate::delivery::GoogleDelivery::new(
        &recording.service_account_json,
        &recording.gcs_bucket,
        &recording.drive_id,
        std::sync::Arc::new(|| 0),
    )
    .map(|_| ())
    .map_err(|error| format!("CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON: {error}"))
}

pub fn recording_needs_verified_identity(recording: bool, oauth: bool) -> Result<(), String> {
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

/// `CODETRIAL_ALLOWED_HOSTS`, a comma-separated list of `Host` values.
///
/// Unset or empty means no check. A value that holds only separators is
/// refused instead: it was meant to name a host, and reading it as no list
/// would answer every `Host` the person believes is refused.
pub fn allowed_hosts(values: &BTreeMap<String, String>) -> Result<Vec<String>, String> {
    let hosts: Vec<String> = value_or(values, "CODETRIAL_ALLOWED_HOSTS", "")
        .split(',')
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_string)
        .collect();
    if hosts.is_empty() && nonempty(values, "CODETRIAL_ALLOWED_HOSTS").is_some() {
        return Err("CODETRIAL_ALLOWED_HOSTS names no Host".to_string());
    }
    Ok(hosts)
}

/// Unset is zero hops; a value that is not a count is an error, since reading a
/// typo as zero would treat a server behind a proxy as a local run.
fn trusted_proxy_hops(values: &BTreeMap<String, String>) -> Result<u32, String> {
    match nonempty(values, "CODETRIAL_TRUSTED_PROXY_HOPS") {
        None => Ok(0),
        Some(value) => value
            .parse()
            .map_err(|_| "CODETRIAL_TRUSTED_PROXY_HOPS must be a whole number".to_string()),
    }
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
pub fn relaxed_for_local_use(values: &BTreeMap<String, String>) -> Vec<String> {
    if crate::config::is_production(values) {
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
#[derive(Clone, Copy)]
pub enum Reachable {
    /// The bind address is not loopback.
    Address,
    /// `CODETRIAL_TRUSTED_PROXY_HOPS` is the operator saying requests arrive
    /// through something in front, which is the same admission: a server on
    /// loopback behind nginx or a tunnel is as public as one on `0.0.0.0`.
    DeclaredProxy,
}

pub fn reachable_from_elsewhere(
    values: &BTreeMap<String, String>,
    bound: std::net::SocketAddr,
) -> Option<Reachable> {
    if !bound.ip().to_canonical().is_loopback() {
        Some(Reachable::Address)
    } else if !matches!(trusted_proxy_hops(values), Ok(0)) {
        Some(Reachable::DeclaredProxy)
    } else {
        None
    }
}

pub fn task_mode_allowed(reachable: Option<Reachable>, fixed_room: bool, recording: bool) -> bool {
    task_mode_refusal(reachable, fixed_room, recording).is_none()
}

/// Why task mode may not run on this server, in the words of what to change, or
/// `None` where it may.
pub fn task_mode_refusal(
    reachable: Option<Reachable>,
    fixed_room: bool,
    recording: bool,
) -> Option<&'static str> {
    match (reachable, fixed_room, recording) {
        (Some(Reachable::Address), _, _) => Some(
            "it is bound to an address other than loopback; start it with --web-addr 127.0.0.1:3000",
        ),
        (Some(Reachable::DeclaredProxy), _, _) => {
            Some("CODETRIAL_TRUSTED_PROXY_HOPS says requests arrive through a proxy; unset it")
        }
        (None, true, _) => Some("INTERVIEW_ROOM_NAME shares one fixed room; unset it"),
        (None, false, true) => {
            Some("recording is on; task mode does not run on a recording server")
        }
        (None, false, false) => None,
    }
}

/// Task mode is on when `--task-mode` asked for it and the server is one it may
/// run on; either alone is not enough.
pub fn task_mode_enabled(
    requested: bool,
    reachable: Option<Reachable>,
    fixed_room: bool,
    recording: bool,
) -> bool {
    requested && task_mode_allowed(reachable, fixed_room, recording)
}

pub fn published_secret_refusal(
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
    crate::config::compiler_explorer_enabled(
        values
            .get("CODETRIAL_COMPILER_EXPLORER_ENABLED")
            .map(String::as_str),
    )
}

pub fn guest_mode_recording_conflict(guest_mode_enabled: bool, recording_enabled: bool) -> bool {
    guest_mode_enabled && recording_enabled
}

pub fn guest_mode_enabled(values: &BTreeMap<String, String>) -> bool {
    crate::config::guest_mode_enabled(
        values
            .get("CODETRIAL_GUEST_MODE_ENABLED")
            .map(String::as_str),
    )
}

pub fn nonempty(values: &BTreeMap<String, String>, key: &str) -> Option<String> {
    values
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn value_or(values: &BTreeMap<String, String>, key: &str, default: &str) -> String {
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
pub fn account_db_path(values: &BTreeMap<String, String>, options: &CliOptions) -> PathBuf {
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
pub fn cold_start_config_path(options: &CliOptions) -> PathBuf {
    if let Some(dir) = &options.data_dir {
        return dir.join(CONFIG_FILE_NAME);
    }
    let in_checkout = PathBuf::from(CONFIG_DIR);
    if in_checkout.join(CONFIG_EXAMPLE_NAME).is_file() {
        return in_checkout.join(CONFIG_FILE_NAME);
    }
    crate::exe_dir().join(CONFIG_DIR).join(CONFIG_FILE_NAME)
}

/// What a launch settles before it binds a socket: the refusals and warnings
/// that depend only on the values, the provider pool, the server config and
/// the account database.
pub struct Prepared {
    pub config: WebServerConfig,
    pub max_concurrent: usize,
}

pub fn prepare(
    values: &BTreeMap<String, String>,
    options: &CliOptions,
) -> Result<Prepared, String> {
    if let Some(refusal) = production_secret_refusal(values) {
        return Err(refusal);
    }
    for warning in relaxed_for_local_use(values) {
        eprintln!("{warning}");
    }

    // Beside the other startup warnings and not next to the dispatcher that
    // consumes it. A cap the operator wrote and this cannot honor is worth
    // saying before the first thing that can fail: read late, a bad bind or a
    // half-configured pool ends the process first and the operator never learns
    // the number in their config file was ignored.
    let max_concurrent = read_max_concurrent(values);

    let production = crate::config::is_production(values);
    let pool = web_provider_pool(values, options, production)?;

    let config = web_server_config(values, pool, production, options)?;

    if guest_mode_recording_conflict(config.guest_mode_enabled, config.recording.is_some()) {
        eprintln!(
            "WARNING: CODETRIAL_GUEST_MODE_ENABLED=true is ignored while recording is enabled; \\
             guest mode is unavailable"
        );
    }

    initialize_accounts(&config)?;
    Ok(Prepared {
        config,
        max_concurrent,
    })
}

/// Who started this server. A launcher that owns it (the desktop app, a test
/// fixture) offers no Setup page; an operator who started it is offered one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchedBy {
    Parent,
    Operator,
}

/// What a launch settles once its socket is bound and its address is known:
/// the published-secret refusal, the Setup page and task mode, both of which
/// depend on where the server is reachable.
pub fn finish(
    mut config: WebServerConfig,
    values: &BTreeMap<String, String>,
    options: &CliOptions,
    bound: std::net::SocketAddr,
    launched_by: LaunchedBy,
) -> Result<(WebServerConfig, Option<crate::web::SetupContext>), String> {
    if let Some(refusal) = published_secret_refusal(values, bound) {
        return Err(refusal);
    }
    let setup = crate::web::SetupContext::new(
        primary_config_path(options)?,
        values.clone(),
        bound,
        launched_by == LaunchedBy::Operator,
        config.trusted_proxy_hops,
    )?;
    // Same reason `serve_setup` prints this: say where to go.
    println!("codetrial: open http://{bound} in your browser");
    if let Some(setup) = &setup {
        println!(
            "codetrial: to change keys at http://{bound}/setup, enter {}",
            setup.code()
        );
    }

    // Task mode is off unless `--task-mode` asks for it. It serves the learner
    // at this machine and nobody else: it holds decrypted reference solutions
    // in memory and trusts its page to report the test rules. Reachable from
    // anywhere else, or set up to record or to share one fixed room, its routes
    // say why they are off. A declared proxy makes a loopback bind as public as
    // any other.
    let reachable = reachable_from_elsewhere(values, bound);
    let (fixed_room, recording) = (config.fixed_room_name.is_some(), config.recording.is_some());
    config.tasks = task_mode_enabled(config.task_mode_requested, reachable, fixed_room, recording)
        .then(crate::tasks::access::TaskService::new);

    // Said at startup, because the address printed above is the interview lobby
    // on any other server, and a learner who asked for task mode would not
    // otherwise know what to open or why nothing changed.
    if config.tasks.is_some() {
        println!(
            "codetrial: task mode is on; open http://{bound} for the default task, or the assignment link your instructor sent"
        );
    } else if config.task_mode_requested
        && let Some(reason) = task_mode_refusal(reachable, fixed_room, recording)
    {
        eprintln!("codetrial: --task-mode is off: {reason}");
    }

    Ok((config, setup))
}

pub fn local_agent_config(
    values: BTreeMap<String, String>,
) -> Result<Option<crate::config::AgentConfig>, String> {
    // Whether this process also hosts interviewers. A full agent config, which
    // is a Gemini key on top of what the web side needs, means yes; without it
    // this is the web half of a split deployment and agents arrive from
    // elsewhere. Deciding it by what is configured, rather than by a flag,
    // keeps a single-host production deployment working out of the box while
    // leaving the split one exactly as it was. No `extend_pool` here on
    // purpose. The dispatcher never looks a provider up: it is handed the one
    // the token was minted from, so a second scan could only introduce a pool
    // that disagrees with the web side's.
    //
    // Only keys that are absent make it the web half. An entry that is present
    // but invalid is a mistake in this host's config, and serving on without
    // the local interviewer would leave every interview waiting forever.
    let agent_config = match crate::config::load_from_pairs(values) {
        Ok(config) => Some(config),
        Err(error) if error.invalid_entries.is_empty() => None,
        Err(error) => return Err(error.to_string()),
    };
    if agent_config.is_none() {
        eprintln!(
            "no GOOGLE_API_KEY or GOOGLE_API_KEYS: serving the web side only. Interviews will wait forever unless \
             an agent joins each room from elsewhere."
        );
    }
    Ok(agent_config)
}

/// How long a stopping server may wait on what it still holds.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bounds {
    /// Open connections holding the shutdown up once it has begun (`None`
    /// waits for them): a client holding a half-sent request would otherwise
    /// keep a fixture from ever reporting.
    pub drain: Option<std::time::Duration>,
    /// Admitted rooms and the reports they are still writing, after the last
    /// connection has gone and nothing new can be admitted (`None` does not
    /// wait, and ends them when the runtime drops).
    pub settle: Option<std::time::Duration>,
}

/// Serves the web service, and the local interviewer when there is an agent
/// config, until `shutdown` resolves and the connections in flight have
/// finished. `codetrial web` passes a future that never resolves and no
/// bounds; a fixture passes its stop signal and bounds. The listener must
/// already be non-blocking. The runtime is dropped before this returns, which
/// ends any room task still running.
pub fn serve(
    listener: std::net::TcpListener,
    config: WebServerConfig,
    setup: Option<crate::web::SetupContext>,
    agent_config: Option<crate::config::AgentConfig>,
    max_concurrent: usize,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    bounds: Bounds,
) -> Result<(), String> {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should start");
    let dispatcher = agent_config.map(|config| {
        std::sync::Arc::new(crate::dispatch::LocalDispatcher {
            config,
            runtime: runtime.handle().clone(),
            state: std::sync::Arc::default(),
            max_concurrent,
        }) as std::sync::Arc<dyn crate::web::RoomDispatcher>
    });
    runtime.block_on(run(listener, config, setup, dispatcher, shutdown, bounds))
}

/// `serve` with the dispatcher supplied, for a caller that has its own.
pub fn serve_with(
    listener: std::net::TcpListener,
    config: WebServerConfig,
    setup: Option<crate::web::SetupContext>,
    dispatcher: Option<std::sync::Arc<dyn crate::web::RoomDispatcher>>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    bounds: Bounds,
) -> Result<(), String> {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime should start");
    runtime.block_on(run(listener, config, setup, dispatcher, shutdown, bounds))
}

async fn run(
    listener: std::net::TcpListener,
    config: WebServerConfig,
    setup: Option<crate::web::SetupContext>,
    dispatcher: Option<std::sync::Arc<dyn crate::web::RoomDispatcher>>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    bounds: Bounds,
) -> Result<(), String> {
    let listener = tokio::net::TcpListener::from_std(listener)
        .map_err(|error| format!("web server failed: {error}"))?;
    let watched = dispatcher.clone();
    let closing = dispatcher.clone();
    let (begun, shutting_down) = tokio::sync::oneshot::channel::<()>();
    let served = axum::serve(
        listener,
        crate::web::web_service_with_setup(config, dispatcher, setup),
    )
    .with_graceful_shutdown(async move {
        shutdown.await;

        // Closing the listener does not end requests already in flight, and one
        // of them could still admit a room while the settle wait below is
        // looking at an idle dispatcher.
        if let Some(dispatcher) = closing {
            dispatcher.close_admission();
        }
        let _ = begun.send(());
    });
    let overdue = async {
        match (shutting_down.await, bounds.drain) {
            (Ok(()), Some(bound)) => tokio::time::sleep(bound).await,
            _ => std::future::pending().await,
        }
    };
    let served = tokio::select! {
        result = served => result,
        () = overdue => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "connections were still open when the shutdown bound passed",
        )),
    }
    .map_err(|error| format!("web server failed: {error}"));

    // Nothing new is admitted now. Whatever was is allowed to finish, report
    // included, before the runtime drops and ends it.
    match (bounds.settle, watched) {
        (Some(limit), Some(dispatcher)) if !settled(&*dispatcher, limit).await => {
            Err(unfinished(served))
        }
        _ => served,
    }
}

/// Whether the dispatcher went idle within `limit`.
async fn settled(dispatcher: &dyn crate::web::RoomDispatcher, limit: std::time::Duration) -> bool {
    let began = tokio::time::Instant::now();
    while !dispatcher.is_idle() {
        if began.elapsed() >= limit {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    true
}

/// The error of a stop that found rooms or reports still running at the settle
/// bound: that, and the shutdown's own error as well when it had one, since a
/// drain timeout must not hide that evidence was cut off.
fn unfinished(served: Result<(), String>) -> String {
    const UNFINISHED: &str = "rooms or reports were still running when the settle bound passed";
    match served {
        Ok(()) => UNFINISHED.into(),
        Err(error) => format!("{error}; {UNFINISHED}"),
    }
}

#[cfg(test)]
#[path = "../tests/unit/launch.rs"]
mod tests;

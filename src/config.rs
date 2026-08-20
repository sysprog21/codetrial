use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

pub const DEFAULT_GEMINI_LIVE_MODEL: &str = "gemini-3.1-flash-live-preview";

/// How long Gemini waits for silence before deciding the candidate has finished
/// speaking. Left unset the API picks its own value, and the interview measured
/// nearly five seconds between a candidate stopping and the reply starting,
/// with
/// an empty playout queue: that wait is endpointing plus inference, and this is
/// the half we can move.
///
/// A knob rather than a constant because the right value is a property of the
/// room and the speaker, not of the code. Someone who pauses mid-sentence to
/// think needs a longer window than someone who does not, and cutting it too
/// short interrupts people while they are still talking.
pub const DEFAULT_GEMINI_SILENCE_MS: u32 = 700;

/// The clamp exists for a typo, not for an API limit. A previous version of
/// this said "Gemini accepts at most two seconds" and capped at 2000, which is
/// wrong: setup was measured accepting 2001, 5000 and 30000 against the live
/// endpoint. Capping there silently discarded the middle of the range this knob
/// exists to explore, which is the failure mode a tuning knob can least afford.
///
/// Thirty seconds is not a limit Gemini imposes either. It is the point past
/// which the value is a mistake: an interviewer that waits half a minute before
/// answering is broken whatever the API thinks.
pub const MAX_GEMINI_SILENCE_MS: u32 = 30_000;
pub const DEFAULT_GEMINI_REPORT_MODEL: &str = "gemini-3.1-flash-lite";
pub const DEFAULT_GEMINI_VOICE: &str = "Puck";
pub const DEFAULT_ROOM_PREFIX: &str = "interview";
pub const DEFAULT_DURATION_MIN: u32 = 45;
/// Interview length the token endpoint and the agent both clamp to, so a
/// hand-crafted request cannot book a 10-hour room or a 1-minute one. The
/// browser lobby mirrors this range in `web/interview.js`; the server clamp is
/// the enforcing one.
pub const MIN_DURATION_MIN: u32 = 10;
pub const MAX_DURATION_MIN: u32 = 90;
pub const DEFAULT_WEB_DIR: &str = "web";
pub const DEFAULT_WEB_ADDR: &str = "127.0.0.1:3000";
pub const DEFAULT_COMPILER_EXPLORER_ENABLED: bool = true;
pub const DEFAULT_GEMINI_CANDIDATE_VIDEO_ENABLED: bool = false;

const REQUIRED_KEYS: &[&str] = &[
    "LIVEKIT_URL",
    "LIVEKIT_API_KEY",
    "LIVEKIT_API_SECRET",
    "GOOGLE_API_KEY",
];

/// The credentials that came from the process environment, or from
/// `config/codetrial.env.local`. Rooms served by it carry no provider segment,
/// so a single-provider deployment keeps the room names it always had.
pub const PRIMARY_PROVIDER_ID: &str = "primary";

const PROVIDER_FILE_PREFIX: &str = "codetrial.env.";

/// One LiveKit project and the Gemini key that goes with it. The Google key
/// lives here rather than in a parallel list because the two are chosen
/// together: a parallel list means two independent indexes into two vectors
/// that can differ in length, which is a whole class of routing bug that stops
/// existing once there is one record to look up.
#[derive(Clone, PartialEq, Eq)]
pub struct Provider {
    /// `primary`, or the suffix of a `config/codetrial.env.<id>` file. This is
    /// what travels inside the room name: it is the only channel the web
    /// process has to tell a separately launched agent which LiveKit project
    /// the candidate was handed a token for.
    pub id: String,
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
    pub google_api_key: String,
}

/// The policy for credentials is "not printable". `AgentConfig` gets that by
/// having no `Debug` at all, which is the stronger form because the compiler
/// keeps it true. `Provider` cannot: `ProviderPool` and `WebServerConfig` both
/// derive `Debug`, so it needs an impl, and this one redacts. Add a secret
/// field here and it must be added below too, which is exactly why the other
/// type does not take this approach.
impl fmt::Debug for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Provider")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("api_key", &"<redacted>")
            .field("api_secret", &"<redacted>")
            .field("google_api_key", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderPool {
    /// Primary first, then discovered providers sorted by id. `read_dir` order
    /// is unspecified, and sorting is what makes round-robin fairness and the
    /// skipped-file warnings the same on every run. Cross-process agreement no
    /// longer rides on this order: the room name carries the provider id.
    pub providers: Vec<Provider>,
}

impl ProviderPool {
    pub fn primary(&self) -> Option<&Provider> {
        self.providers.first()
    }

    pub fn get(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|provider| provider.id == id)
    }

    /// Round robin for real, not a hash pretending to be one. The room name
    /// records which provider won, so the choice does not have to be
    /// reproducible from the room name by a second process.
    pub fn select(&self, counter: usize) -> Option<&Provider> {
        if self.providers.is_empty() {
            None
        } else {
            Some(&self.providers[counter % self.providers.len()])
        }
    }

    /// The provider a room belongs to. This is the whole feature: the web
    /// process picks a provider and writes its id into the room name, and the
    /// agent process, which is handed nothing but that name, resolves the same
    /// record. Both sides call this rather than each spelling out
    /// parse-then-look-up-then-fall-back, because two copies of that rule are
    /// two chances to send the candidate and the agent to different projects.
    pub fn for_room(&self, room_name: &str, room_prefix: &str) -> Option<&Provider> {
        provider_id_from_room(room_name, room_prefix)
            .and_then(|id| self.get(id))
            .or_else(|| self.primary())
    }
}

/// The provider segment of `<prefix>-<id>-<suffix>`. `<prefix>-<suffix>`, which
/// is what a single-provider deployment mints, has none.
pub fn provider_id_from_room<'a>(room_name: &'a str, room_prefix: &str) -> Option<&'a str> {
    room_name
        .strip_prefix(room_prefix)?
        .strip_prefix('-')?
        .split_once('-')
        .map(|(id, _)| id)
}

/// Provider ids end up inside room names, so they get the same alphabet as the
/// room suffix and no separator can appear in one. `primary` is taken by the
/// credentials from the environment.
fn is_provider_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 16
        && id
            .chars()
            .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
        && id != PRIMARY_PROVIDER_ID
}

/// `local` is the operator's own primary config and `example` is the
/// checked-in template. Both live in this directory on purpose, so neither is
/// worth a word.
const EXPECTED_NON_PROVIDER_IDS: [&str; 2] = ["local", "example"];

/// Extra LiveKit projects, one per `config/codetrial.env.<id>`, sorted by id.
///
/// Pure in its directory argument: nothing here reads the process environment
/// or the current directory, so a test can point it at a fixture and two
/// binaries can be told to look at the same place instead of each guessing from
/// wherever they happened to be started.
///
/// A file that does not parse, or that names only part of a provider, is
/// skipped with a warning rather than failing the load. An optional extra
/// project must not be able to stop the primary one from serving.
pub fn discover_providers(dir: &Path, production: bool) -> (Vec<Provider>, Vec<String>) {
    let mut warnings = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (Vec::new(), warnings);
    };

    // Anything named like a provider file that is not going to be loaded says
    // so. Dropping it silently is how an operator ends up with a project they
    // believe is in the pool and rooms that never reach it.
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name.strip_prefix(PROVIDER_FILE_PREFIX) else {
            continue;
        };
        if EXPECTED_NON_PROVIDER_IDS.contains(&id) {
            continue;
        }
        if !is_provider_id(id) {
            warnings.push(format!(
                "{}: skipped, a provider id must be 1 to 16 lowercase letters or digits and cannot be {PRIMARY_PROVIDER_ID}",
                entry.path().display()
            ));
            continue;
        }

        // `file_type` does not follow symlinks, so a link dropped into the
        // config directory cannot make the server read a file outside it.
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            warnings.push(format!(
                "{}: skipped, not a regular file",
                entry.path().display()
            ));
            continue;
        }
        files.push((id.to_string(), entry.path()));
    }
    files.sort();

    let providers = files
        .into_iter()
        .filter_map(
            |(id, path)| match provider_from_file(&id, &path, production) {
                Ok(provider) => Some(provider),
                Err(warning) => {
                    warnings.push(warning);
                    None
                }
            },
        )
        .collect();
    (providers, warnings)
}

fn provider_from_file(id: &str, path: &Path, production: bool) -> Result<Provider, String> {
    let values = read_config_file(path)
        .map_err(|message| format!("{}: skipped, {message}", path.display()))?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    let value = |key: &str| {
        values
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    };
    let (Some(url), Some(api_key), Some(api_secret)) = (
        value("LIVEKIT_URL"),
        value("LIVEKIT_API_KEY"),
        value("LIVEKIT_API_SECRET"),
    ) else {
        return Err(format!(
            "{}: skipped, a provider file needs LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET",
            path.display()
        ));
    };
    validate_livekit_url(url, production)
        .map_err(|message| format!("{}: skipped, {message}", path.display()))?;
    Ok(Provider {
        id: id.to_string(),
        url: url.to_string(),
        api_key: api_key.to_string(),
        api_secret: api_secret.to_string(),
        google_api_key: value("GOOGLE_API_KEY").unwrap_or_default().to_string(),
    })
}

/// One parser for every `KEY=value` file this binary reads, so a line accepted
/// for the primary config is accepted for a provider file too.
pub fn read_config_file(path: &Path) -> Result<Vec<(String, String)>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut values = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(format!("invalid config line: {trimmed}"));
        };
        values.push((
            key.trim().to_string(),
            value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string(),
        ));
    }
    Ok(values)
}

/// No `Debug`, deliberately: two of these fields are credentials, and nothing
/// needs to print the struct. See the `Debug` impl on [`Provider`] for why that
/// type is handled the other way.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentConfig {
    pub livekit_url: String,
    pub livekit_api_key: String,
    pub livekit_api_secret: String,
    pub google_api_key: String,
    pub gemini_live_model: String,
    pub gemini_report_model: String,
    pub gemini_voice: String,
    pub gemini_silence_ms: u32,
    pub room_prefix: String,
    pub default_duration_min: u32,
    pub web_dir: String,
    pub web_addr: String,
    pub compiler_explorer_enabled: bool,
    pub gemini_candidate_video_enabled: bool,
    pub pool: ProviderPool,
}

/// Scheme, and whether it encrypts. A LiveKit join token is a bearer
/// credential, so plaintext is a local-development convenience and nothing
/// more.
const LIVEKIT_SCHEMES: [(&str, bool); 4] = [
    ("wss://", true),
    ("https://", true),
    ("ws://", false),
    ("http://", false),
];

/// Rejects what `url_origin` in `web.rs` would otherwise have to cope with: a
/// scheme nothing here knows, or a scheme with no host after it. The URL never
/// appears in the message, because the message reaches stderr and a LiveKit URL
/// can carry a query string.
pub fn validate_livekit_url(url: &str, production: bool) -> Result<(), String> {
    let trimmed = url.trim();
    let Some((scheme, encrypted)) = LIVEKIT_SCHEMES.iter().find(|(scheme, _)| {
        trimmed
            .get(..scheme.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
    }) else {
        return Err("LIVEKIT_URL must start with wss://, https://, ws:// or http://".to_string());
    };
    if trimmed[scheme.len()..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .is_empty()
    {
        return Err("LIVEKIT_URL has a scheme but no host".to_string());
    }
    if production && !encrypted {
        return Err("LIVEKIT_URL must use wss:// or https:// when NODE_ENV=production".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub missing_keys: Vec<&'static str>,
    pub invalid_entries: Vec<String>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.missing_keys.is_empty() {
            write!(
                formatter,
                "missing required config keys: {}",
                self.missing_keys.join(", ")
            )?;
        }
        if !self.invalid_entries.is_empty() {
            if !self.missing_keys.is_empty() {
                write!(formatter, "; ")?;
            }
            write!(
                formatter,
                "invalid config entries: {}",
                self.invalid_entries.join(", ")
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

pub fn load_from_pairs(
    pairs: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
) -> Result<AgentConfig, ConfigError> {
    let values: BTreeMap<String, String> = pairs
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
    let missing_keys = REQUIRED_KEYS
        .iter()
        .copied()
        .filter(|key| values.get(*key).is_none_or(|value| value.trim().is_empty()))
        .collect::<Vec<_>>();

    let livekit_url = optional(&values, "LIVEKIT_URL", "");
    let production = is_production(&values);

    // `CODETRIAL_WEB_ADDR` is deliberately not validated here.
    // `bind_web_listener` resolves it through `ToSocketAddrs`, which accepts
    // hostnames, and it is the only place the value is used: a check that only
    // accepts a literal `SocketAddr` would reject `localhost:3000` and would
    // fire in the two modes that never bind a listener at all.
    let mut invalid_entries = Vec::new();
    if !livekit_url.is_empty()
        && let Err(message) = validate_livekit_url(&livekit_url, production)
    {
        invalid_entries.push(message);
    }

    if !missing_keys.is_empty() || !invalid_entries.is_empty() {
        return Err(ConfigError {
            missing_keys,
            invalid_entries,
        });
    }

    // `optional` with an empty default, not a lookup that panics when the key
    // is absent: the check above already returned, and if it ever stops doing
    // so the result is an empty credential that LiveKit rejects rather than a
    // panic at a call site that cannot see why it was meant to be safe.
    let livekit_api_key = optional(&values, "LIVEKIT_API_KEY", "");
    let livekit_api_secret = optional(&values, "LIVEKIT_API_SECRET", "");
    let google_api_key = optional(&values, "GOOGLE_API_KEY", "");

    // Only the primary. Discovering the rest reads the filesystem, which is the
    // caller's business: keeping it out of here is what lets a test call this
    // function and get the same answer whatever directory it runs in.
    let pool = ProviderPool {
        providers: vec![Provider {
            id: PRIMARY_PROVIDER_ID.to_string(),
            url: livekit_url.clone(),
            api_key: livekit_api_key.clone(),
            api_secret: livekit_api_secret.clone(),
            google_api_key: google_api_key.clone(),
        }],
    };

    Ok(AgentConfig {
        livekit_url,
        livekit_api_key,
        livekit_api_secret,
        google_api_key,
        gemini_live_model: optional(&values, "GEMINI_LIVE_MODEL", DEFAULT_GEMINI_LIVE_MODEL),
        gemini_report_model: report_model_or_default(&values),
        gemini_voice: optional(&values, "GEMINI_VOICE", DEFAULT_GEMINI_VOICE),
        gemini_silence_ms: optional_u32(&values, "GEMINI_SILENCE_MS", DEFAULT_GEMINI_SILENCE_MS)
            .min(MAX_GEMINI_SILENCE_MS),
        room_prefix: optional(&values, "CODETRIAL_ROOM_PREFIX", DEFAULT_ROOM_PREFIX),
        default_duration_min: optional_u32(&values, "CODETRIAL_DURATION_MIN", DEFAULT_DURATION_MIN),
        web_dir: optional(&values, "CODETRIAL_WEB_DIR", DEFAULT_WEB_DIR),
        web_addr: optional(&values, "CODETRIAL_WEB_ADDR", DEFAULT_WEB_ADDR),
        compiler_explorer_enabled: compiler_explorer_enabled(
            values
                .get("CODETRIAL_COMPILER_EXPLORER_ENABLED")
                .map(String::as_str),
        ),
        gemini_candidate_video_enabled: gemini_candidate_video_enabled(
            values
                .get("CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED")
                .map(String::as_str),
        ),
        pool,
    })
}

/// One reading of "is this a production deployment", because it is what refuses
/// a plaintext `LIVEKIT_URL`. Two spellings of it would be one edit away from
/// disagreeing about when a join token may cross the wire in the clear.
pub fn is_production(values: &BTreeMap<String, String>) -> bool {
    values
        .get("NODE_ENV")
        .is_some_and(|value| value.trim() == "production")
}

fn optional(values: &BTreeMap<String, String>, key: &str, default: &str) -> String {
    values
        .get(key)
        .filter(|value| !value.trim().is_empty())
        .map_or_else(|| default.to_string(), |value| value.trim().to_string())
}

fn report_model_or_default(values: &BTreeMap<String, String>) -> String {
    match optional(values, "GEMINI_REPORT_MODEL", DEFAULT_GEMINI_REPORT_MODEL).as_str() {
        "gemini-flash-latest" | "gemini-2.5-flash" | "models/gemini-2.5-flash" => {
            DEFAULT_GEMINI_REPORT_MODEL.to_string()
        }
        model => model.to_string(),
    }
}

fn optional_u32(values: &BTreeMap<String, String>, key: &str, default: u32) -> u32 {
    values
        .get(key)
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}

fn parse_bool(value: Option<&str>, default: bool) -> bool {
    match value.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if value.is_empty() => default,
        Some(value) if matches!(value.as_str(), "1" | "true" | "yes" | "on") => true,
        Some(value) if matches!(value.as_str(), "0" | "false" | "no" | "off") => false,
        Some(_) => false,
        None => default,
    }
}

pub fn compiler_explorer_enabled(value: Option<&str>) -> bool {
    parse_bool(value, DEFAULT_COMPILER_EXPLORER_ENABLED)
}

pub fn gemini_candidate_video_enabled(value: Option<&str>) -> bool {
    parse_bool(value, DEFAULT_GEMINI_CANDIDATE_VIDEO_ENABLED)
}

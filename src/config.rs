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

/// Production recording, or its absence.
///
/// One struct rather than a dozen loose keys because the values are only
/// meaningful together: a bucket with no service account cannot be written to,
/// and a template URL with no Egress project cannot be fetched by anything.
/// Absent means recording is off, which is the default and the state every
/// deployment is in until an operator provisions the resources in
/// `docs/recording-contract.md`.
#[derive(Clone, PartialEq, Eq)]
pub struct RecordingConfig {
    /// `None` means record on whichever LiveKit project owns the room, which
    /// is how rooms are routed today. `Some` names one of the pool's projects
    /// with its own credentials, for a key scoped to `roomRecord`.
    pub livekit: Option<RecordingLivekit>,
    pub gcs_bucket: String,
    pub gcs_prefix: String,
    pub drive_id: String,
    pub service_account_json: String,
    pub max_minutes: u32,
    /// Kilobits per second, the unit `EncodingOptions.video_bitrate` uses.
    pub bitrate: u32,
    pub kill_switch: bool,
    pub template_base_url: String,
    pub timeout_seconds: u64,
    pub integration: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct RecordingLivekit {
    pub url: String,
    pub api_key: String,
    pub api_secret: String,
}

/// Redacting, for the same reason [`Provider`] redacts: `WebServerConfig`
/// derives `Debug`, so anything reachable from it can reach a log line. The
/// service-account key is the worst of these, because it is a private key in a
/// string field and it would print in full.
impl fmt::Debug for RecordingConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecordingConfig")
            .field("livekit", &self.livekit)
            .field("gcs_bucket", &self.gcs_bucket)
            .field("gcs_prefix", &self.gcs_prefix)
            .field("drive_id", &self.drive_id)
            .field("service_account_json", &"<redacted>")
            .field("max_minutes", &self.max_minutes)
            .field("bitrate", &self.bitrate)
            .field("kill_switch", &self.kill_switch)
            .field("template_base_url", &self.template_base_url)
            .field("timeout_seconds", &self.timeout_seconds)
            .field("integration", &self.integration)
            .finish()
    }
}

impl fmt::Debug for RecordingLivekit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The host, not the URL. `validate_livekit_url` accepts a query string
        // and userinfo, which is exactly why no message in this file prints a
        // LiveKit URL, and `Debug` is a message like any other.
        formatter
            .debug_struct("RecordingLivekit")
            .field("host", &host_of(livekit_authority(&self.url)))
            .field("api_key", &"<redacted>")
            .field("api_secret", &"<redacted>")
            .finish()
    }
}

/// The authority of a URL, or the whole string when it has no scheme. Shared by
/// the redaction above and the project comparison below, so that neither can
/// mistake a credential for part of a hostname.
fn livekit_authority(url: &str) -> &str {
    url.split_once("://")
        .map_or(url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default()
}

/// Matches `DEFAULT_DURATION_MIN`, and validation refuses anything below the
/// configured interview length: a recording that stops before the interview
/// does produces an artifact that is missing exactly the part a reviewer would
/// have watched it for.
pub const DEFAULT_RECORDING_MAX_MINUTES: u32 = DEFAULT_DURATION_MIN;
/// Kilobits per second. The contract's ceiling, written down in one place.
pub const DEFAULT_RECORDING_BITRATE: u32 = crate::recording::OUTPUT_VIDEO_BITRATE;
/// Bounds, not preferences. Below the floor the 720p output is unwatchable and
/// the recording is worthless; above the ceiling an operator has quietly
/// tripled the transcode bill for a talking-head video.
pub const MIN_RECORDING_BITRATE: u32 = 200;
pub const MAX_RECORDING_BITRATE: u32 = 8_000;
pub const DEFAULT_RECORDING_GCS_PREFIX: &str = "codetrial";
pub const DEFAULT_RECORDING_TIMEOUT_SECONDS: u64 = 900;

/// The credentials that only make sense together. Naming them as a group is
/// what makes a half-configured deployment fail at startup instead of at the
/// moment a candidate's interview ends.
const RECORDING_REQUIRED_KEYS: &[&str] = &[
    "CODETRIAL_RECORDING_GCS_BUCKET",
    "CODETRIAL_RECORDING_DRIVE_ID",
    "CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON",
    "CODETRIAL_RECORDING_TEMPLATE_BASE_URL",
];

/// The LiveKit override. All three or none: a URL with no secret is a project
/// nothing can call, and a secret with no URL is a credential for nowhere.
const RECORDING_LIVEKIT_KEYS: [&str; 3] = [
    "CODETRIAL_RECORDING_LIVEKIT_URL",
    "CODETRIAL_RECORDING_LIVEKIT_API_KEY",
    "CODETRIAL_RECORDING_LIVEKIT_API_SECRET",
];

/// Reads the recording block, or reports every reason it cannot be used.
///
/// Separate from [`load_from_pairs`] because the two callers disagree about
/// what else must be present: `codetrial serve` insists on a Gemini key, and
/// `codetrial web` deliberately does not. Both need this, and neither should
/// have to reimplement it.
///
/// Errors carry key names and reasons, never values. A validation message is a
/// log line, and a log line holding a service-account key is the same incident
/// as committing one.
pub fn load_recording(
    values: &BTreeMap<String, String>,
    pool: &ProviderPool,
    interview_duration_min: u32,
    production: bool,
) -> Result<Option<RecordingConfig>, ConfigError> {
    // The switch is read strictly and on its own, in that order. Strictly,
    // because `CODETRIAL_RECORDING_ENABLED=ture` would otherwise turn recording
    // off silently: the safe direction, arrived at by accident, with no consent
    // prompt, no artifact, and nothing said. On its own, because everything
    // below really is ignored while recording is off, and a deployment that
    // does not record must not fail to start over a typo in a value nothing
    // reads.
    let mut invalid_entries = Vec::new();
    if !recording_flag(values, "CODETRIAL_RECORDING_ENABLED", &mut invalid_entries)
        || !invalid_entries.is_empty()
    {
        return if invalid_entries.is_empty() {
            Ok(None)
        } else {
            Err(ConfigError {
                missing_keys: Vec::new(),
                invalid_entries,
            })
        };
    }
    let kill_switch = recording_flag(
        values,
        "CODETRIAL_RECORDING_KILL_SWITCH",
        &mut invalid_entries,
    );
    let integration = recording_flag(
        values,
        "CODETRIAL_RECORDING_INTEGRATION",
        &mut invalid_entries,
    );

    let present = |key: &str| {
        values
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    };
    let mut missing_keys = RECORDING_REQUIRED_KEYS
        .iter()
        .copied()
        .filter(|key| present(key).is_none())
        .collect::<Vec<_>>();

    let configured_livekit = RECORDING_LIVEKIT_KEYS
        .iter()
        .filter(|key| present(key).is_some())
        .count();
    if configured_livekit != 0 && configured_livekit != RECORDING_LIVEKIT_KEYS.len() {
        missing_keys.extend(
            RECORDING_LIVEKIT_KEYS
                .iter()
                .copied()
                .filter(|key| present(key).is_none()),
        );
    }

    let livekit = (configured_livekit == RECORDING_LIVEKIT_KEYS.len()).then(|| RecordingLivekit {
        url: present("CODETRIAL_RECORDING_LIVEKIT_URL")
            .unwrap_or_default()
            .to_string(),
        api_key: present("CODETRIAL_RECORDING_LIVEKIT_API_KEY")
            .unwrap_or_default()
            .to_string(),
        api_secret: present("CODETRIAL_RECORDING_LIVEKIT_API_SECRET")
            .unwrap_or_default()
            .to_string(),
    });
    if let Some(livekit) = &livekit {
        if let Err(message) = validate_livekit_url(&livekit.url, production) {
            invalid_entries.push(format!("CODETRIAL_RECORDING_LIVEKIT_URL: {message}"));
        } else if !pool
            .providers
            .iter()
            .any(|provider| same_livekit_host(&provider.url, &livekit.url))
        {
            // Egress runs inside the project that holds the room, so recording
            // against a project this server never hands a token for records
            // nothing. The message names no URL: a LiveKit URL can carry a
            // query string, and this one reaches stderr.
            invalid_entries.push(
                "CODETRIAL_RECORDING_LIVEKIT_URL names a LiveKit project that is not in the \
                 provider pool, so no room this server creates would ever be recorded"
                    .to_string(),
            );
        }
    }

    if let Some(url) = present("CODETRIAL_RECORDING_TEMPLATE_BASE_URL")
        && let Err(message) = validate_template_base_url(url)
    {
        invalid_entries.push(format!("CODETRIAL_RECORDING_TEMPLATE_BASE_URL: {message}"));
    }

    let max_minutes = recording_number(
        values,
        "CODETRIAL_RECORDING_MAX_MINUTES",
        DEFAULT_RECORDING_MAX_MINUTES,
        &mut invalid_entries,
    );
    if max_minutes < interview_duration_min {
        invalid_entries.push(format!(
            "CODETRIAL_RECORDING_MAX_MINUTES is {max_minutes}, below the {interview_duration_min} \
             minute interview, so a recording would stop before the interview does"
        ));
    }

    // The credential is parsed here, not at the first delivery. A key that
    // cannot be read is a deployment that records interviews it can never hand
    // over, and finding that out an hour into the first one is finding it out
    // from a candidate.
    if let Some(service_account) = present("CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON")
        && let Err(message) = crate::delivery::readable_service_account(service_account)
    {
        invalid_entries.push(format!(
            "CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON: {message}"
        ));
    }

    let bitrate = recording_number(
        values,
        "CODETRIAL_RECORDING_BITRATE",
        DEFAULT_RECORDING_BITRATE,
        &mut invalid_entries,
    );
    if !(MIN_RECORDING_BITRATE..=MAX_RECORDING_BITRATE).contains(&bitrate) {
        invalid_entries.push(format!(
            "CODETRIAL_RECORDING_BITRATE is {bitrate} kbps, outside \
             {MIN_RECORDING_BITRATE}..={MAX_RECORDING_BITRATE}"
        ));
    }

    let timeout_seconds = recording_number(
        values,
        "CODETRIAL_RECORDING_TIMEOUT_SECONDS",
        DEFAULT_RECORDING_TIMEOUT_SECONDS,
        &mut invalid_entries,
    );
    if timeout_seconds == 0 {
        invalid_entries
            .push("CODETRIAL_RECORDING_TIMEOUT_SECONDS must be at least one second".to_string());
    }

    if !missing_keys.is_empty() || !invalid_entries.is_empty() {
        return Err(ConfigError {
            missing_keys,
            invalid_entries,
        });
    }

    Ok(Some(RecordingConfig {
        livekit,
        gcs_bucket: present("CODETRIAL_RECORDING_GCS_BUCKET")
            .unwrap_or_default()
            .to_string(),
        gcs_prefix: optional(
            values,
            "CODETRIAL_RECORDING_GCS_PREFIX",
            DEFAULT_RECORDING_GCS_PREFIX,
        ),
        drive_id: present("CODETRIAL_RECORDING_DRIVE_ID")
            .unwrap_or_default()
            .to_string(),
        service_account_json: present("CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON")
            .unwrap_or_default()
            .to_string(),
        max_minutes,
        bitrate,
        kill_switch,
        template_base_url: present("CODETRIAL_RECORDING_TEMPLATE_BASE_URL")
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string(),
        timeout_seconds,
        integration,
    }))
}

/// A recording number, where a value that is present and unparseable is an
/// error rather than a silent fall back to the default.
///
/// `optional_u32` defaults on a parse failure, which is right for a tuning knob
/// and wrong here: an operator who wrote `CODETRIAL_RECORDING_BITRATE=2 Mbps`
/// would get a server that started, recorded, and billed at whatever the
/// default happened to be.
fn recording_number<T: std::str::FromStr>(
    values: &BTreeMap<String, String>,
    key: &'static str,
    default: T,
    invalid_entries: &mut Vec<String>,
) -> T {
    let Some(value) = values
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return default;
    };
    match value.parse() {
        Ok(parsed) => parsed,
        Err(_) => {
            // The value is not echoed. Every other message in this function
            // names keys only, and one exception is how the habit dies.
            invalid_entries.push(format!("{key} is not a number"));
            default
        }
    }
}

/// A recording boolean, under the same rule: present and unrecognized is an
/// error. `parse_bool` reads anything it does not know as false, which for a
/// feature that must fail closed means failing closed silently.
fn recording_flag(
    values: &BTreeMap<String, String>,
    key: &'static str,
    invalid_entries: &mut Vec<String>,
) -> bool {
    let Some(value) = values
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => {
            invalid_entries.push(format!("{key} must be true or false"));
            false
        }
    }
}

/// LiveKit Cloud Egress fetches the RoomComposite template over the public
/// internet, so a loopback or private address is not a URL it can reach. This
/// is the same rule `scripts/recording-provision-check.sh` applies to the
/// value an operator records, kept in step deliberately: an operator who
/// passed the provisioning check should not then fail at startup.
///
/// The ceiling, because this is not an SSRF guard and nothing here fetches the
/// URL: a literal address is classified by parsing it, but `0x7f000001`,
/// `2130706433` and a hostname that resolves privately all read as ordinary
/// hosts and are accepted. What this catches is the mistake an operator
/// actually makes, which is pointing Egress at the machine they are sitting
/// at. Catching the rest would mean resolving names at startup and again
/// before every fetch, for a value only an operator can set.
fn validate_template_base_url(url: &str) -> Result<(), String> {
    let Some(rest) = url.strip_prefix("https://") else {
        return Err(
            "must start with https://, because Egress fetches it from the public internet"
                .to_string(),
        );
    };
    if rest.contains('?') || rest.contains('#') {
        return Err("must be an origin and path only; Egress appends its own query".to_string());
    }
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return Err("has a scheme but no host".to_string());
    }

    // Refused rather than stripped. Userinfo in a URL is a credential, and this
    // value is printed by `Debug` and pasted into an Egress request; a password
    // that survives redaction because it was hiding inside a URL is the leak
    // nobody looks for.
    if authority.contains('@') {
        return Err(
            "must not carry userinfo; a credential inside a URL is a credential in a log line"
                .to_string(),
        );
    }
    let Some((host, port)) = split_authority(authority) else {
        return Err("has a malformed host".to_string());
    };
    if host.is_empty() {
        return Err("has a scheme but no host".to_string());
    }
    if let Some(port) = port
        && port.parse::<u16>().ok().is_none_or(|port| port == 0)
    {
        return Err("has a port Egress cannot connect to".to_string());
    }
    if is_unreachable_host(host) {
        return Err(
            "must not be a loopback or private address; Egress cannot reach it".to_string(),
        );
    }
    Ok(())
}

/// An authority split into host and port, with the brackets off an IPv6
/// literal.
///
/// Splitting on a colon anywhere is wrong for `[::1]:443`, and splitting on the
/// last one is wrong for `[::1]` on its own, so the bracketed form is handled
/// first and separately. An unbracketed IPv6 literal is not a legal authority
/// and comes back with an empty host, which every caller already refuses.
fn split_authority(authority: &str) -> Option<(&str, Option<&str>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        // An opening bracket with no closing one, or anything after the closing
        // one that is not a port, is not an authority. Treating `[2001:db8::1`
        // as a host would let a truncated URL validate.
        let (host, tail) = rest.split_once(']')?;
        let port = match tail {
            "" => None,
            tail => Some(tail.strip_prefix(':')?),
        };
        return Some((host, port));
    }
    Some(match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    })
}

fn host_of(authority: &str) -> &str {
    split_authority(authority).map_or("", |(host, _)| host)
}

/// Whether a host names this machine or a network Egress cannot route to.
///
/// Parsed rather than prefix-matched. The prefix version this replaced rejected
/// every hostname beginning `fc` or `fd`, which includes `facebook.com`, and
/// let `[::ffff:127.0.0.1]` straight through.
fn is_unreachable_host(host: &str) -> bool {
    // One trailing dot, which is the fully qualified spelling of the same name.
    // `localhost.` resolves to loopback like `localhost` does.
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    let Ok(address) = host.parse::<std::net::IpAddr>() else {
        return false;
    };

    // An IPv4 address wearing an IPv6 costume is still that address.
    let address = match address {
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, std::net::IpAddr::V4),
        other => other,
    };
    match address {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        std::net::IpAddr::V6(v6) => {
            // `is_unique_local` and `is_unicast_link_local` are still unstable,
            // so fc00::/7 and fe80::/10 are matched on the bits rather than
            // waited for.
            let leading = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || leading & 0xfe00 == 0xfc00
                || leading & 0xffc0 == 0xfe80
        }
    }
}

/// Whether two LiveKit URLs name the same project.
///
/// Scheme is ignored because `wss://host` and `https://host` are one endpoint,
/// but the port is not: two projects on one host would be two ports. The
/// default port is normalized away, so `wss://host` and `https://host:443` are
/// recognized as the same place rather than reported as a misconfiguration.
pub fn same_livekit_project(left: &str, right: &str) -> bool {
    same_livekit_host(left, right)
}

fn same_livekit_host(left: &str, right: &str) -> bool {
    fn authority(url: &str) -> Option<String> {
        let (scheme, rest) = url.split_once("://")?;
        let authority = rest.split(['/', '?', '#']).next()?;

        // Userinfo is dropped rather than compared. It is a credential, not
        // part of the endpoint's identity, and two URLs for one project may
        // legitimately carry different ones.
        let authority = authority.rsplit('@').next()?;
        let (host, port) = split_authority(authority)?;
        let host = host.to_ascii_lowercase();
        if host.is_empty() {
            return None;
        }
        let default_port = match scheme.to_ascii_lowercase().as_str() {
            "wss" | "https" => "443",
            "ws" | "http" => "80",
            _ => "",
        };
        let port = port.filter(|port| !port.is_empty()).unwrap_or(default_port);
        Some(format!("{host}:{port}"))
    }
    match (authority(left), authority(right)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

mod providers;
pub use providers::{
    PRIMARY_PROVIDER_ID, PROVIDER_ORDER_KEY, Provider, ProviderPool, discover_providers,
    order_providers, pool_summary, provider_id_from_room,
};

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
///
/// Every reply to speech the candidate did not yield waits out this window
/// before Gemini starts, so it is the largest share of reply latency. Three
/// seconds made each such reply arrive about 3.4s after the last word, against
/// 0.8s when the candidate yielded through the browser control, which ends the
/// audio stream instead. A second and a half halves that wait and still covers
/// a breath between clauses; a longer pause to think is what the thinking hold
/// is for, and the page draws the window filling, so the wait is visible
/// rather than a surprise. Rooms whose speakers pause longer can still raise
/// `GEMINI_SILENCE_MS`.
pub const DEFAULT_GEMINI_SILENCE_MS: u32 = 1_500;

/// How readily Gemini decides the candidate has started speaking, and so how
/// readily it abandons a reply it is part way through delivering.
///
/// `LOW` because the interviewer is usually the one talking, and the cost of
/// the two mistakes is not symmetric. Missing a real interruption for a moment
/// is a candidate waiting a beat to be heard. Taking a cough, a keystroke or
/// the interviewer's own voice returning through an open speaker as an
/// interruption throws away a reply already generated: one measured session
/// discarded 5.9 seconds of speech mid-sentence, twice as often as it finished
/// a turn.
///
/// A knob rather than a constant for the same reason the silence window is one.
/// The right value is a property of the room and the hardware, and a candidate
/// on headphones can afford to be interrupted more eagerly than one on a laptop
/// speaker.
pub const DEFAULT_GEMINI_START_SENSITIVITY: &str = "START_SENSITIVITY_LOW";

/// How readily Gemini decides the candidate has finished, before the silence
/// window starts counting. `HIGH` is the API's documented default, so sending
/// it changes nothing today; it is named so that the window was measured
/// against a known value and a different provider default cannot move it
/// unseen. An operator who sees slow speakers cut off can set
/// `GEMINI_END_SENSITIVITY=LOW` or raise the window.
pub const DEFAULT_GEMINI_END_SENSITIVITY: &str = "END_SENSITIVITY_HIGH";

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
/// Guest interviews require explicit operator opt-in.
pub const DEFAULT_GUEST_MODE_ENABLED: bool = false;
/// The update notice asks GitHub about the `latest` release while a lobby is
/// open, with nobody signing in and no account on the request, where GitHub
/// sign-in reaches GitHub only for a candidate who signs in. So it waits to be
/// asked for.
pub const DEFAULT_UPDATE_CHECK: bool = false;
/// Off until measured in real interviews: a short spoken acknowledgement
/// played while a tool round holds the reply back. See `gemini::tool_ack_clip`.
pub const DEFAULT_GEMINI_TOOL_ACK: bool = false;

/// How many interviews one `codetrial web` process will host agents for at
/// once. Each is a LiveKit room plus a metered Gemini Live session, so an
/// unbounded count is an unbounded bill; a refused dispatch leaves the
/// candidate on the "Waiting" pill, which is bad, but recoverable and visible.
///
/// Configurable because it is the one number that tracks the operator's budget
/// and hardware rather than anything this code knows.
pub const DEFAULT_MAX_CONCURRENT_INTERVIEWS: usize = 16;

/// Interim reviews use the report model's quota before the final report does.
/// Six four-line notes fill half the final report's note budget, which the
/// report does not need full: it reads the whole transcript and the code
/// itself, and each review is a call of about five hundred tokens. Zero is
/// useful to an operator who must reserve a shared key for final reports.
pub const DEFAULT_MAX_INTERIM_REVIEWS: usize = 6;
pub const MAX_INTERIM_REVIEWS: usize = 72;

/// How long an owed reply may go without output before the Live socket is
/// replaced. Issue 95 logged a 39-second reply on a degraded provider that did
/// arrive, so the default sits above it; a reply the watchdog replaces is paid
/// for twice. Bounded below by the twenty seconds after which an unanswered
/// prompt already hands the floor back, since a shorter timeout would replace
/// sockets a held `GoAway` is still waiting on, and above so that a silent
/// provider is still recovered while the candidate is waiting for it.
pub const DEFAULT_GEMINI_REPLY_TIMEOUT_S: u32 = 45;
pub const MIN_GEMINI_REPLY_TIMEOUT_S: u32 = 20;
pub const MAX_GEMINI_REPLY_TIMEOUT_S: u32 = 120;

/// How long a reply nothing has started toward may go with nothing back before
/// the Live socket is replaced. A working socket answers well inside the
/// default, even if only with the end of a turn that chose silence; one that
/// hangs sends nothing. Never longer than the reply timeout, which bounds every
/// wait, and bounded below so a slow but working start is not replaced.
pub const DEFAULT_GEMINI_FIRST_OUTPUT_S: u32 = 10;
pub const MIN_GEMINI_FIRST_OUTPUT_S: u32 = 5;

const REQUIRED_KEYS: &[&str] = &[
    "LIVEKIT_URL",
    "LIVEKIT_API_KEY",
    "LIVEKIT_API_SECRET",
    "GOOGLE_API_KEY",
];

/// One parser for every `KEY=value` file this binary reads, so a line accepted
/// for the primary config is accepted for a provider file too.
pub fn read_config_file(path: &Path) -> Result<Vec<(String, String)>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;

    // By number, never by its text: the message reaches the console, a log file
    // and a launcher's window, and a malformed line can hold a secret.
    parse_config(&text)
        .map_err(|number| format!("{}: line {number} is not KEY=value", path.display()))
}

/// The pairs in `text`, or the number, counted from 1, of the first line that
/// is not `KEY=value`. Apart from `read_config_file` for a caller that has to
/// say which line without repeating it: the line can hold a secret.
pub(crate) fn parse_config(text: &str) -> Result<Vec<(String, String)>, usize> {
    // A byte order mark from an editor that saves UTF-8 with one. `trim` does
    // not remove it, so left in place it joins the first key or hides a `#`.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut values = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(index + 1);
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

/// Lays the config file over the environment. The two Gemini credential names
/// are one setting: a file that sets either replaces both, so an inherited
/// `GOOGLE_API_KEYS` cannot outrank the file's own `GOOGLE_API_KEY`. A blank
/// entry is no credential, so it leaves the other name alone.
pub fn apply_config_file(values: &mut BTreeMap<String, String>, file: Vec<(String, String)>) {
    if file.iter().any(|(key, value)| {
        (key == "GOOGLE_API_KEY" || key == "GOOGLE_API_KEYS") && !value.trim().is_empty()
    }) {
        values.remove("GOOGLE_API_KEY");
        values.remove("GOOGLE_API_KEYS");
    }
    values.extend(file);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GeminiContextCompression {
    pub trigger_tokens: u32,
    pub target_tokens: u32,
}

fn gemini_context_compression(
    values: &BTreeMap<String, String>,
) -> Result<Option<GeminiContextCompression>, String> {
    let trigger = present(values, "GEMINI_CONTEXT_TRIGGER_TOKENS");
    let target = present(values, "GEMINI_CONTEXT_TARGET_TOKENS");
    let (Some(trigger), Some(target)) = (trigger, target) else {
        return if trigger.is_none() && target.is_none() {
            Ok(None)
        } else {
            Err("GEMINI_CONTEXT_TRIGGER_TOKENS and GEMINI_CONTEXT_TARGET_TOKENS must be set together".into())
        };
    };
    let parse = |value: &str, name: &str| {
        value
            .parse::<u32>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("{name} must be a positive integer"))
    };
    let trigger_tokens = parse(trigger, "GEMINI_CONTEXT_TRIGGER_TOKENS")?;
    let target_tokens = parse(target, "GEMINI_CONTEXT_TARGET_TOKENS")?;
    if target_tokens >= trigger_tokens {
        return Err(
            "GEMINI_CONTEXT_TARGET_TOKENS must be less than GEMINI_CONTEXT_TRIGGER_TOKENS".into(),
        );
    }
    Ok(Some(GeminiContextCompression {
        trigger_tokens,
        target_tokens,
    }))
}

/// No `Debug`, deliberately: two of these fields are credentials, and nothing
/// needs to print the struct. See the `Debug` impl on [`Provider`] for why that
/// type is handled the other way.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentConfig {
    pub livekit_url: String,
    pub livekit_api_key: String,
    pub livekit_api_secret: String,
    pub google_api_keys: Vec<String>,
    pub gemini_live_model: String,
    pub gemini_report_model: String,
    pub gemini_voice: String,
    pub gemini_silence_ms: u32,
    pub gemini_context_compression: Option<GeminiContextCompression>,
    pub gemini_start_sensitivity: String,
    pub gemini_end_sensitivity: String,
    pub room_prefix: String,
    pub default_duration_min: u32,
    pub gemini_candidate_video_enabled: bool,
    pub gemini_tool_ack: bool,
    pub max_interim_reviews: usize,
    pub gemini_reply_timeout_s: u32,
    pub gemini_first_output_s: u32,
    pub pool: ProviderPool,
}

/// Scheme, whether it encrypts, and the scheme a RoomService request to the
/// same deployment is made over. A LiveKit join token is a bearer credential,
/// so plaintext is a local-development convenience and nothing more.
///
/// The third column lives here rather than beside the two functions that
/// rewrite a URL with it, because all three lists have to agree on what a
/// scheme is and two of them already disagreed: this one matched `WSS://`
/// case-insensitively and `livekit_http_base` did not, so an accepted URL was
/// posted to as `WSS://`, which is not a scheme reqwest will send.
const LIVEKIT_SCHEMES: [(&str, bool, &str); 4] = [
    ("wss://", true, "https://"),
    ("https://", true, "https://"),
    ("ws://", false, "http://"),
    ("http://", false, "http://"),
];

/// The entry `url` starts with, matched the way a URL scheme compares: without
/// regard to case. Case is the whole reason this is a function and not an
/// `iter().find()` at each call site.
pub(crate) fn livekit_scheme(url: &str) -> Option<&'static (&'static str, bool, &'static str)> {
    LIVEKIT_SCHEMES.iter().find(|(scheme, _, _)| {
        url.get(..scheme.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
    })
}

/// The HTTP API base for a LiveKit URL: the same host, over the scheme an
/// HTTP client will send.
///
/// The rewrite goes through `livekit_scheme` rather than matching the two
/// websocket schemes here, because `validate_livekit_url` accepts a scheme in
/// any case and this has to rewrite every spelling that accepts. A pasted
/// `WSS://` used to survive unchanged, and reqwest refuses any scheme but http
/// and https before the request leaves the process, so credentials that were
/// good came back as credentials that did not work.
///
/// The query and the fragment come off, because the Twirp path is appended
/// to what this returns and validation accepts both. A configured
/// `?region=eu` took the method name into the query rather than to the
/// service. The path itself is kept: a LiveKit behind a reverse proxy at
/// `https://host/livekit` serves RoomService under that prefix.
pub(crate) fn livekit_http_base(url: &str) -> String {
    let trimmed = url.trim();
    let trimmed = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    let trimmed = trimmed.trim_end_matches('/');
    match livekit_scheme(trimmed) {
        Some((scheme, _, http)) => format!("{http}{}", &trimmed[scheme.len()..]),
        None => trimmed.to_string(),
    }
}

pub(crate) fn livekit_host_is_csp_safe(authority: &str) -> bool {
    let Some((host, port)) = split_authority(authority) else {
        return false;
    };
    let host_is_safe = if authority.starts_with('[') {
        host.parse::<std::net::Ipv6Addr>().is_ok()
    } else {
        !host.is_empty()
            && host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    };
    host_is_safe && port.is_none_or(|port| port.parse::<u16>().is_ok_and(|port| port != 0))
}

/// Rejects what the URL's consumers would otherwise have to cope with: an
/// unknown scheme, no host, or host syntax that cannot become a CSP source.
/// The URL never appears in the message, because the message reaches stderr
/// and a LiveKit URL can carry a query string.
pub fn validate_livekit_url(url: &str, production: bool) -> Result<(), String> {
    let trimmed = url.trim();
    let Some((scheme, encrypted, _)) = livekit_scheme(trimmed) else {
        return Err("LIVEKIT_URL must start with wss://, https://, ws:// or http://".to_string());
    };
    let host = trimmed[scheme.len()..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    if host.is_empty() {
        return Err("LIVEKIT_URL has a scheme but no host".to_string());
    }
    if !livekit_host_is_csp_safe(host) {
        return Err("LIVEKIT_URL host contains invalid characters".to_string());
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
        .filter(|key| *key != "GOOGLE_API_KEY" || present(&values, "GOOGLE_API_KEYS").is_none())
        .filter(|key| values.get(*key).is_none_or(|value| value.trim().is_empty()))
        .collect::<Vec<_>>();

    let livekit_url = optional(&values, "LIVEKIT_URL", "");
    let production = is_production(&values);

    let mut invalid_entries = Vec::new();
    let google_api_keys = google_api_keys(&values).unwrap_or_else(|message| {
        invalid_entries.push(message);
        Vec::new()
    });
    if !livekit_url.is_empty()
        && let Err(message) = validate_livekit_url(&livekit_url, production)
    {
        invalid_entries.push(message);
    }

    let gemini_context_compression =
        gemini_context_compression(&values).unwrap_or_else(|message| {
            invalid_entries.push(message);
            None
        });

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

    // Only the primary. Discovering the rest reads the filesystem, which is the
    // caller's business: keeping it out of here is what lets a test call this
    // function and get the same answer whatever directory it runs in.
    let pool = ProviderPool {
        providers: vec![Provider {
            id: PRIMARY_PROVIDER_ID.to_string(),
            url: livekit_url.clone(),
            api_key: livekit_api_key.clone(),
            api_secret: livekit_api_secret.clone(),
            google_api_keys: google_api_keys.clone(),
        }],
    };

    let gemini_reply_timeout_s = optional_u32(
        &values,
        "CODETRIAL_GEMINI_REPLY_TIMEOUT_S",
        DEFAULT_GEMINI_REPLY_TIMEOUT_S,
    )
    .clamp(MIN_GEMINI_REPLY_TIMEOUT_S, MAX_GEMINI_REPLY_TIMEOUT_S);
    Ok(AgentConfig {
        livekit_url,
        livekit_api_key,
        livekit_api_secret,
        google_api_keys,
        gemini_live_model: optional(&values, "GEMINI_LIVE_MODEL", DEFAULT_GEMINI_LIVE_MODEL),
        gemini_report_model: report_model_or_default(&values),
        gemini_voice: optional(&values, "GEMINI_VOICE", DEFAULT_GEMINI_VOICE),
        gemini_silence_ms: optional_u32(&values, "GEMINI_SILENCE_MS", DEFAULT_GEMINI_SILENCE_MS)
            .min(MAX_GEMINI_SILENCE_MS),
        gemini_context_compression,
        gemini_start_sensitivity: sensitivity_or_default(
            &values,
            "START_SENSITIVITY",
            DEFAULT_GEMINI_START_SENSITIVITY,
        ),
        gemini_end_sensitivity: sensitivity_or_default(
            &values,
            "END_SENSITIVITY",
            DEFAULT_GEMINI_END_SENSITIVITY,
        ),
        room_prefix: optional(&values, "CODETRIAL_ROOM_PREFIX", DEFAULT_ROOM_PREFIX),
        default_duration_min: optional_u32(&values, "CODETRIAL_DURATION_MIN", DEFAULT_DURATION_MIN),
        gemini_candidate_video_enabled: gemini_candidate_video_enabled(
            values
                .get("CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED")
                .map(String::as_str),
        ),
        gemini_tool_ack: parse_bool(
            values.get("CODETRIAL_GEMINI_TOOL_ACK").map(String::as_str),
            DEFAULT_GEMINI_TOOL_ACK,
        ),
        max_interim_reviews: optional_u32(
            &values,
            "CODETRIAL_MAX_INTERIM_REVIEWS",
            DEFAULT_MAX_INTERIM_REVIEWS as u32,
        )
        .min(MAX_INTERIM_REVIEWS as u32) as usize,
        gemini_reply_timeout_s,
        gemini_first_output_s: optional_u32(
            &values,
            "CODETRIAL_GEMINI_FIRST_OUTPUT_S",
            DEFAULT_GEMINI_FIRST_OUTPUT_S,
        )
        .clamp(MIN_GEMINI_FIRST_OUTPUT_S, gemini_reply_timeout_s),
        pool,
    })
}

/// The Gemini keys in the order to try them: the comma-separated
/// `GOOGLE_API_KEYS`, or else `GOOGLE_API_KEY` alone. A blank value is absence.
/// Resolved here so nothing downstream chooses between the two names.
pub fn google_api_keys(values: &BTreeMap<String, String>) -> Result<Vec<String>, String> {
    let Some(value) = present(values, "GOOGLE_API_KEYS") else {
        return Ok(present(values, "GOOGLE_API_KEY")
            .map(str::to_string)
            .into_iter()
            .collect());
    };
    let mut keys = Vec::new();
    for key in value.split(',').map(str::trim) {
        if key.is_empty() || keys.iter().any(|seen| seen == key) {
            return Err("GOOGLE_API_KEYS must contain non-empty, distinct keys".to_string());
        }
        keys.push(key.to_string());
    }
    if present(values, "GOOGLE_API_KEY").is_some() {
        eprintln!("GOOGLE_API_KEYS takes precedence over GOOGLE_API_KEY");
    }
    Ok(keys)
}

/// One reading of "is this a production deployment", because it is what refuses
/// a plaintext `LIVEKIT_URL`. Two spellings of it would be one edit away from
/// disagreeing about when a join token may cross the wire in the clear.
pub fn is_production(values: &BTreeMap<String, String>) -> bool {
    values
        .get("NODE_ENV")
        .is_some_and(|value| value.trim() == "production")
}

/// The trimmed value of `key`, or `None` when it is absent or blank.
///
/// One definition of "set", because every caller below needs the same one and a
/// second spelling of it is a second answer to whether `KEY=" "` counts.
fn present<'a>(values: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    values
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

fn optional(values: &BTreeMap<String, String>, key: &str, default: &str) -> String {
    present(values, key).map_or_else(|| default.to_string(), str::to_string)
}

/// The two values the API names, and nothing else.
///
/// A typo here is worse than a rejected config: the field is one Gemini ignores
/// when it cannot read it, so `START_SENSITIVITY_LOWW` would silently restore
/// the eager default this exists to move away from, and the only evidence would
/// be an interviewer that gets cut off again for no stated reason. The short
/// spellings are accepted because the long ones are shouted API constants and
/// nobody types them twice.
///
/// `prefix` is `START_SENSITIVITY` or `END_SENSITIVITY`, so `GEMINI_<prefix>`
/// is the key and `<prefix>_LOW` or `<prefix>_HIGH` the value sent.
fn sensitivity_or_default(
    values: &BTreeMap<String, String>,
    prefix: &str,
    default: &str,
) -> String {
    let key = format!("GEMINI_{prefix}");
    let value = optional(values, &key, default).to_ascii_uppercase();
    let level = value
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('_'))
        .unwrap_or(&value);
    match level {
        "LOW" | "HIGH" => format!("{prefix}_{level}"),
        _ => {
            eprintln!("{key}: {value} is not LOW or HIGH; using {default}");
            default.to_string()
        }
    }
}

fn report_model_or_default(values: &BTreeMap<String, String>) -> String {
    match optional(values, "GEMINI_REPORT_MODEL", DEFAULT_GEMINI_REPORT_MODEL).as_str() {
        "gemini-flash-latest" | "gemini-2.5-flash" | "models/gemini-2.5-flash" => {
            DEFAULT_GEMINI_REPORT_MODEL.to_string()
        }
        model => model.to_string(),
    }
}

/// `CODETRIAL_MAX_CONCURRENT_INTERVIEWS`, or the default, and a word about it
/// when the operator asked for something this cannot do.
///
/// The fallback does not fail the load: the cap is a throttle, and refusing to
/// start over a typo in it would trade a slow server for no server. It does not
/// happen quietly either. Zero and `sixteeen` both land on the default, and an
/// operator who set one of them deliberately, to drain a node before a deploy,
/// would otherwise get sixteen interviews and no hint that the number they
/// wrote was never read.
pub fn max_concurrent_interviews(
    values: &BTreeMap<String, String>,
    warnings: &mut Vec<String>,
) -> usize {
    const KEY: &str = "CODETRIAL_MAX_CONCURRENT_INTERVIEWS";
    let Some(configured) = values
        .get(KEY)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return DEFAULT_MAX_CONCURRENT_INTERVIEWS;
    };

    // Matched on the literal rather than guarded on `parsed > 0`. The guard
    // says the same thing through a comparison, and a comparison is four more
    // ways for the mutation gate to ask whether anything checks it.
    match configured.parse::<usize>() {
        // Zero is spelled out separately. "must be a positive whole number" is
        // true of it too, but an operator who typed 0 meant "stop taking
        // interviews", and being told their number was unreadable would send
        // them looking for a typo that is not there.
        Ok(0) => {
            warnings.push(format!(
                "{KEY}=0 would refuse every interview; using {DEFAULT_MAX_CONCURRENT_INTERVIEWS}. \
                 Stop the process to drain it."
            ));
            DEFAULT_MAX_CONCURRENT_INTERVIEWS
        }
        Ok(parsed) => parsed,
        Err(_) => {
            warnings.push(format!(
                "{KEY}={configured} is not a positive whole number; using \
                 {DEFAULT_MAX_CONCURRENT_INTERVIEWS}."
            ));
            DEFAULT_MAX_CONCURRENT_INTERVIEWS
        }
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

pub fn guest_mode_enabled(value: Option<&str>) -> bool {
    parse_bool(value, DEFAULT_GUEST_MODE_ENABLED)
}

/// `CODETRIAL_EXIT_WITH_STDIN`: off unless a launcher asks for it.
pub fn exit_with_stdin(value: Option<&str>) -> bool {
    parse_bool(value, false)
}

pub fn gemini_candidate_video_enabled(value: Option<&str>) -> bool {
    parse_bool(value, DEFAULT_GEMINI_CANDIDATE_VIDEO_ENABLED)
}

/// The commit `CODETRIAL_UPDATE_CHECK` compares against the `latest` release,
/// or `None` when nothing is compared: the setting is off, the binary was not
/// built by CI (`release_commit` is empty, see `CODETRIAL_RELEASE_COMMIT` in
/// build.rs), or the platform does not support the notice.
///
/// The platform is an argument rather than a `cfg!` here so the rule can be
/// tested on the Linux runners that run the suite.
pub fn update_check_commit(
    value: Option<&str>,
    release_commit: &str,
    supported_platform: bool,
) -> Option<String> {
    (parse_bool(value, DEFAULT_UPDATE_CHECK) && supported_platform && !release_commit.is_empty())
        .then(|| release_commit.to_string())
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
/// the redaction above, the project comparison below and Setup's offer, so
/// that none of them can mistake a credential for part of a hostname.
pub(crate) fn livekit_authority(url: &str) -> &str {
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

/// A recording has to outlast the interview it records: `stale_after` in
/// recording/sweeper.rs reaps one that outlives this cap, ending the recording
/// before the interview. `recording_config` refuses a configured deployment
/// that gets this wrong; the compiler refuses the defaults, which is worth the
/// four lines because the two are one expression apart today and a literal in
/// place of that expression would otherwise be found in production.
const _: () = assert!(
    DEFAULT_DURATION_MIN <= DEFAULT_RECORDING_MAX_MINUTES,
    "the default recording cap is below the default interview length"
);
/// Kilobits per second. The contract's ceiling, written down in one place.
pub const DEFAULT_RECORDING_BITRATE: u32 = crate::recording::OUTPUT_VIDEO_BITRATE;
/// Bounds, not preferences. Below the floor the 720p output is unwatchable and
/// the recording is worthless; above the ceiling an operator has quietly
/// tripled the transcode bill for a talking-head video.
pub const MIN_RECORDING_BITRATE: u32 = 200;
pub const MAX_RECORDING_BITRATE: u32 = 8_000;
pub const DEFAULT_RECORDING_GCS_PREFIX: &str = "codetrial";

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
/// Separate from [`load_from_pairs`], which insists on a Gemini key that a
/// web-only deployment has no use for. The recording block is needed either
/// way, so it is read on its own rather than tied to a config the caller may
/// not be able to build.
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

    let present = |key: &str| present(values, key);
    let mut missing_keys = RECORDING_REQUIRED_KEYS
        .iter()
        .copied()
        .filter(|key| present(key).is_none())
        .collect::<Vec<_>>();

    let livekit = recording_livekit(
        values,
        pool,
        production,
        &mut missing_keys,
        &mut invalid_entries,
    );

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
    }))
}

/// The separate LiveKit project a recording deployment may run egress in.
///
/// All three keys or none: a half-named project is reported through
/// `missing_keys` rather than silently falling back to the room's own project,
/// because an operator who set two of the three meant to record somewhere else.
/// `None` here is that fallback being correct, not a failure.
fn recording_livekit(
    values: &BTreeMap<String, String>,
    pool: &ProviderPool,
    production: bool,
    missing_keys: &mut Vec<&'static str>,
    invalid_entries: &mut Vec<String>,
) -> Option<RecordingLivekit> {
    let present = |key: &str| present(values, key);

    // Read once and matched on shape. Counting them and then re-reading each
    // value meant three arms defaulting a key the count had just proved was
    // there, which would have written an empty URL rather than failed if the
    // guard above them ever drifted.
    let found = RECORDING_LIVEKIT_KEYS.map(present);

    // None of the three is the common case: record on the project that owns the
    // room. Nothing to report.
    if found.iter().all(Option::is_none) {
        return None;
    }
    let [Some(url), Some(api_key), Some(api_secret)] = found else {
        missing_keys.extend(
            RECORDING_LIVEKIT_KEYS
                .iter()
                .copied()
                .filter(|key| present(key).is_none()),
        );
        return None;
    };

    let livekit = RecordingLivekit {
        url: url.to_string(),
        api_key: api_key.to_string(),
        api_secret: api_secret.to_string(),
    };

    if let Err(message) = validate_livekit_url(&livekit.url, production) {
        invalid_entries.push(format!("CODETRIAL_RECORDING_LIVEKIT_URL: {message}"));
    } else if !pool
        .providers
        .iter()
        .any(|provider| same_livekit_project(&provider.url, &livekit.url))
    {
        // Egress runs inside the project that holds the room, so recording
        // against a project this server never hands a token for records
        // nothing. The message names no URL: a LiveKit URL can carry a query
        // string, and this one reaches stderr.
        invalid_entries.push(
            "CODETRIAL_RECORDING_LIVEKIT_URL names a LiveKit project that is not in the \
             provider pool, so no room this server creates would ever be recorded"
                .to_string(),
        );
    }

    Some(livekit)
}

/// A recording number, where a value that is present and unparseable is an
/// error rather than a silent fall back to the default.
///
/// `optional_u32` defaults on a parse failure, which is right for a tuning knob
/// and wrong here: an operator who wrote `CODETRIAL_RECORDING_BITRATE=2 Mbps`
/// would get a server that started, recorded, and billed at whatever the
/// default happened to be.
fn recording_number(
    values: &BTreeMap<String, String>,
    key: &'static str,
    default: u32,
    invalid_entries: &mut Vec<String>,
) -> u32 {
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
/// internet, so a loopback or private address is not a URL it can reach. The
/// credential-free `scripts/recording-provision-check.sh` is the authoritative
/// DNS-resolution gate: it refuses any origin whose answers are not globally
/// routable before a staging run is provisioned. Startup deliberately keeps a
/// syntactic guard for the recorded value, so it neither depends on DNS nor
/// creates a second, differently timed resolver policy.
///
/// The ceiling, because this is not an SSRF guard and nothing here fetches the
/// URL: a literal address is classified by parsing it, but `0x7f000001`,
/// `2130706433` and a hostname that resolves privately all read as ordinary
/// hosts and are accepted here. The provisioning check catches them before the
/// origin is approved; resolving again at startup and before every fetch would
/// create a second, differently timed DNS policy for an operator-owned value.
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
pub(crate) fn is_unreachable_host(host: &str) -> bool {
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

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
///
/// The default is sized for the speaker this is actually pointed at, who is
/// composing an answer in a second language. 700ms was measured against a
/// fluent speaker and is inside the pause such a candidate takes to find the
/// next word: Gemini called the turn over, Jim answered, and the candidate was
/// still mid-sentence. The cost of the other mistake is that every reply now
/// starts about eight tenths of a second later, which nobody reports as a
/// broken interview.
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

/// How many interviews one `codetrial web` process will host agents for at
/// once. Each is a LiveKit room plus a metered Gemini Live session, so an
/// unbounded count is an unbounded bill; a refused dispatch leaves the
/// candidate on the "Waiting" pill, which is bad, but recoverable and visible.
///
/// Configurable because it is the one number that tracks the operator's budget
/// and hardware rather than anything this code knows.
pub const DEFAULT_MAX_CONCURRENT_INTERVIEWS: usize = 16;

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

/// Provider pooling: the LiveKit projects this deployment can put an interview
/// on, and the record of which one each room went to.
///
/// The feature is the pooling. Round robin is only the policy that picks, and
/// [`ProviderPool::select`] is the one place it lives, so swapping it for
/// least-loaded or weighted touches nothing else.
///
/// The unit is the room, not the connection. Every participant in one interview
/// has to hold credentials for the same project or the candidate and the agent
/// are in two different ones and never meet, which is why the choice is
/// recorded in the room name rather than repeated by whoever needs it next.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderPool {
    /// Primary first, then discovered providers sorted by id, and then whatever
    /// [`order_providers`] was told to lead with. `read_dir` order is
    /// unspecified, and sorting is what makes round-robin fairness and the
    /// skipped-file warnings the same on every run. Nothing may read a meaning
    /// into a position: this is a rotation, and the operator can rotate it.
    /// Cross-process agreement does not ride on the order either, because the
    /// room name carries the provider id.
    pub providers: Vec<Provider>,
}

impl ProviderPool {
    /// The credentials from the environment, which is what a room with no
    /// provider segment was minted by.
    ///
    /// Looked up by id rather than taken from the front, because
    /// `CODETRIAL_PROVIDER_ORDER` can put any provider first. Position used to
    /// carry this meaning, and leaving it that way would mean reordering the
    /// pool silently re-pointed every `<prefix>-<suffix>` room at whichever
    /// project the operator happened to list first.
    ///
    /// Falling back to the front covers the pool built entirely from files,
    /// where there is no environment provider and some project still has to
    /// answer.
    pub fn primary(&self) -> Option<&Provider> {
        self.get(PRIMARY_PROVIDER_ID)
            .or_else(|| self.providers.first())
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

/// The provider segment of `<prefix>-<id>-<suffix>`. Split at the final dash:
/// GitHub-style provider ids may themselves contain dashes. The
/// `<prefix>-<suffix>` a single-provider deployment mints has no segment.
///
/// Every name the pre-dash version of this could mint still parses to the same
/// id, because the suffix alphabet holds no dash and so there was only ever one
/// dash to split on. The one window where the two disagree is a mixed-version
/// deployment that gains a dashed id: an old binary reads
/// `<prefix>-eu-west-<suffix>` as `eu`. It then finds no such project and
/// refuses the room, which is the safe answer, unless a project really is named
/// `eu`. Drain the old binaries before adding a dashed id.
pub fn provider_id_from_room<'a>(room_name: &'a str, room_prefix: &str) -> Option<&'a str> {
    room_name
        .strip_prefix(room_prefix)?
        .strip_prefix('-')?
        .rsplit_once('-')
        .map(|(id, _)| id)
}

/// Provider ids are GitHub usernames, because a provider file normally belongs
/// to its operator. The shape comes from `accounts` rather than being spelled
/// out again: two definitions of "GitHub username" in one crate is one that
/// drifts.
///
/// Stricter than that one in two ways, because this is a name an operator
/// chooses for a file rather than a name a candidate reports about themselves.
/// Consecutive dashes are refused, matching what GitHub accepts today, and
/// nothing legacy has to keep working. `primary` is taken by the credentials
/// from the environment, and reserved in any casing so no file can shadow it.
///
/// Ids are otherwise matched exactly, so a case-insensitive filesystem holds
/// `Foo` or `foo`, never both.
fn is_provider_id(id: &str) -> bool {
    crate::accounts::valid_github_login(id)
        && !id.contains("--")
        && !id.eq_ignore_ascii_case(PRIMARY_PROVIDER_ID)
}

/// Names the providers that lead the rotation, in order, comma separated.
pub const PROVIDER_ORDER_KEY: &str = "CODETRIAL_PROVIDER_ORDER";

/// Puts the providers this names at the front, in the order given.
///
/// Which project leads is an operator decision, not an alphabetical accident.
/// Discovery sorts by id so that a run is reproducible, and that ordering is
/// fine as a default and useless as a policy: it cannot express "spend the
/// account with quota to burn before the one that costs money".
///
/// Anything not named keeps the order it already had and follows. A pool is
/// capacity, so an unlisted provider has to stay in the rotation rather than
/// drop out of it: silently serving from fewer projects than are configured is
/// the failure this whole feature exists to avoid.
///
/// [`ProviderPool::primary`] resolves by id, so wherever a provider called
/// `primary` exists, leading the rotation with another project does not make it
/// the answer for rooms that carry no provider segment. The exception is a pool
/// built entirely from files, which has no `primary` to resolve and falls back
/// to the front: there, and only there, this does move which project answers
/// for a segment-less room.
pub fn order_providers(providers: &mut Vec<Provider>, order: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut leading = Vec::with_capacity(providers.len());
    for name in order
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        // Removed as it is placed, so a name listed twice misses on its second
        // appearance rather than moving the same provider ahead of itself. The
        // two ways to miss are told apart by looking in what has already been
        // placed: one warning covering both would send an operator who mistyped
        // one name hunting for a duplicate that is not there.
        match providers.iter().position(|provider| provider.id == name) {
            Some(index) => leading.push(providers.remove(index)),
            None if leading.iter().any(|provider| provider.id == name) => {
                warnings.push(format!(
                    "{PROVIDER_ORDER_KEY}: {name} is listed more than once; ignoring the repeat"
                ));
            }
            None => warnings.push(format!(
                "{PROVIDER_ORDER_KEY}: no provider is named {name}; ignoring it"
            )),
        }
    }
    leading.append(providers);
    *providers = leading;
    warnings
}

/// The rotation, one line, and a word about any project appearing twice in it.
///
/// Pooling is the one feature whose whole value is invisible from a single
/// interview: it either spread the load or it did not, and nothing a candidate
/// sees says which. An operator adding a project needs to read back what the
/// process actually built, not what the config directory implies.
///
/// Two providers naming one project is the case worth calling out. It is not an
/// error, and different keys on one project are a legitimate thing to hold, but
/// a rotation that visits the same quota twice per cycle is buying less than
/// its length suggests. That happens by accident whenever `--config` names a
/// file that discovery also picks up, which is the ordinary way to point a
/// local run at a second project.
pub fn pool_summary(providers: &[Provider]) -> (String, Vec<String>) {
    let rotation = providers
        .iter()
        .map(|provider| format!("{}={}", provider.id, provider.url))
        .collect::<Vec<_>>()
        .join(", ");

    // Counted here rather than derived from the number of warnings. Deriving it
    // holds only while every repeat produces exactly one line, which is a rule
    // nothing states and that reporting a duplicate against all of its earlier
    // twins rather than the first would break, taking the count negative.
    let mut warnings = Vec::new();
    let mut projects = 0;
    for (index, provider) in providers.iter().enumerate() {
        match providers[..index]
            .iter()
            .find(|earlier| same_livekit_project(&earlier.url, &provider.url))
        {
            Some(earlier) => warnings.push(format!(
                "{} and {} are the same LiveKit project, so the rotation spends two of its turns on one quota",
                earlier.id, provider.id
            )),
            None => projects += 1,
        }
    }

    (
        format!(
            "provider pooling: {} provider(s) over {projects} project(s): {rotation}",
            providers.len()
        ),
        warnings,
    )
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
                "{}: skipped, a provider id must be a GitHub username (1 to 39 letters or digits, single dashes between them) and cannot be {PRIMARY_PROVIDER_ID} in any casing",
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
    pub gemini_start_sensitivity: String,
    pub room_prefix: String,
    pub default_duration_min: u32,
    pub gemini_candidate_video_enabled: bool,
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
        .filter(|key| values.get(*key).is_none_or(|value| value.trim().is_empty()))
        .collect::<Vec<_>>();

    let livekit_url = optional(&values, "LIVEKIT_URL", "");
    let production = is_production(&values);

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
        gemini_start_sensitivity: start_sensitivity_or_default(&values),
        room_prefix: optional(&values, "CODETRIAL_ROOM_PREFIX", DEFAULT_ROOM_PREFIX),
        default_duration_min: optional_u32(&values, "CODETRIAL_DURATION_MIN", DEFAULT_DURATION_MIN),
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
fn start_sensitivity_or_default(values: &BTreeMap<String, String>) -> String {
    match optional(
        values,
        "GEMINI_START_SENSITIVITY",
        DEFAULT_GEMINI_START_SENSITIVITY,
    )
    .to_ascii_uppercase()
    .as_str()
    {
        "LOW" | "START_SENSITIVITY_LOW" => "START_SENSITIVITY_LOW".to_string(),
        "HIGH" | "START_SENSITIVITY_HIGH" => "START_SENSITIVITY_HIGH".to_string(),
        other => {
            eprintln!(
                "GEMINI_START_SENSITIVITY: {other} is not LOW or HIGH; using {DEFAULT_GEMINI_START_SENSITIVITY}"
            );
            DEFAULT_GEMINI_START_SENSITIVITY.to_string()
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
        .any(|provider| same_livekit_host(&provider.url, &livekit.url))
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

//! Provider discovery, room routing and credential-safe diagnostics.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use super::{
    google_api_keys, host_of, livekit_authority, read_config_file, same_livekit_project,
    validate_livekit_url,
};

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
/// Its Gemini keys are one ordered list, whichever name set them; the list
/// does not select another LiveKit project.
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
    pub google_api_keys: Vec<String>,
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
            .field("host", &host_of(livekit_authority(&self.url)))
            .field("api_key", &"<redacted>")
            .field("api_secret", &"<redacted>")
            .field("google_api_keys", &"<redacted>")
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

/// Alphanumerics and dashes, alphanumeric at both ends, 39 bytes at most.
///
/// Deliberately looser than what GitHub accepts at signup today, which also
/// forbids consecutive dashes. That rule is not retroactive, and this gates a
/// handle a candidate types about themselves rather than a name this server
/// derives anything from: a legacy account turned away at the door costs an
/// interview, and letting an odd-looking one through costs nothing, because
/// the handle is a label and never a key. Callers that need the stricter form
/// add it themselves; `is_provider_id` does, for a name it puts in a filename.
pub fn valid_github_login(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 39
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
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
    valid_github_login(id) && !id.contains("--") && !id.eq_ignore_ascii_case(PRIMARY_PROVIDER_ID)
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
        .map(|provider| format!("{}={}", provider.id, livekit_authority(&provider.url)))
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
        google_api_keys: google_api_keys(&values)
            .map_err(|message| format!("{}: skipped, {message}", path.display()))?,
    })
}

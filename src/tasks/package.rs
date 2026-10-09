//! Encrypted task-set packages: the format `scripts/task-package.py` builds,
//! fetched from the instructor's site and opened with the learner's PIN.

use std::collections::{BTreeMap, HashSet};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use ring::{aead, pbkdf2};
use serde::Deserialize;
use serde_json::Value;

use super::access::Assignment;
use super::{TaskError, TaskRecord, default_number, invalid};

/// Bytes of a set's manifest.
const MAX_MANIFEST_BYTES: usize = 4096;
/// Bytes of an instructor's rules note.
const MAX_RULES_NOTE_BYTES: usize = 1024;

/// The rules of one published set version, resolved by the packaging tool
/// and authenticated with the rest of the package.
#[derive(Clone, PartialEq, Eq)]
pub struct SetConfig {
    pub id: String,
    pub version: u32,
    pub closes_at: Option<String>,
    pub rules_note: Option<String>,
    /// The numeric rules; any key absent here takes its default.
    pub rules: BTreeMap<String, u64>,
}

impl SetConfig {
    pub fn rule(&self, key: &str) -> u64 {
        self.rules
            .get(key)
            .copied()
            .unwrap_or_else(|| default_number(key))
    }

    /// The close date as epoch seconds; validation refuses one that does not
    /// parse, so `None` is a set without one.
    pub fn closes_at_seconds(&self) -> Option<u64> {
        self.closes_at
            .as_deref()
            .and_then(|date| parse_close(date).ok())
    }

    pub fn closed(&self, now: u64) -> bool {
        self.closes_at_seconds().is_some_and(|close| close <= now)
    }

    /// The rules as the learner reads them and their result records them.
    pub fn rules_json(&self) -> serde_json::Value {
        serde_json::json!({"durationMin": self.rule("durationMin"),
            "maxHintRungs": self.rule("maxHintRungs"),
            "lookAwaySeconds": self.rule("lookAwaySeconds"),
            "closesAt": self.closes_at, "rulesNote": self.rules_note})
    }
}

/// UTC second precision; no host timezone or permissive date normalization.
/// The one form the packaging tool writes: `YYYY-MM-DDTHH:MM:SSZ`.
pub fn parse_close(text: &str) -> Result<u64, TaskError> {
    let shape = text.len() == 20
        && text.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            10 => byte == b'T',
            13 | 16 => byte == b':',
            19 => byte == b'Z',
            _ => byte.is_ascii_digit(),
        });
    if !shape {
        return Err(invalid("close date must be UTC to the second"));
    }
    let time =
        chrono::DateTime::parse_from_rfc3339(text).map_err(|_| invalid("invalid close date"))?;
    u64::try_from(time.timestamp()).map_err(|_| invalid("close date predates epoch"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    package_version: u32,
    set_id: String,
    set_version: u32,
    salt: String,
    ciphertext_sha256: String,
    ciphertext_bytes: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Rules {
    duration_min: u64,
    max_hint_rungs: u64,
    closes_at: Option<String>,
    look_away_seconds: u64,
    rules_note: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Package {
    package_version: u32,
    set_id: String,
    set_version: u32,
    rules: Rules,
    problems: Vec<Value>,
    judges: BTreeMap<String, Value>,
    variants: BTreeMap<String, Value>,
    sidecars: BTreeMap<String, Value>,
}

/// A decoded set, held in process memory and never written anywhere. The PIN
/// and the key derived from it are gone once this exists.
pub struct LoadedSet {
    pub config: SetConfig,
    pub records: BTreeMap<String, Arc<TaskRecord>>,
}

/// Why a downloaded package did not open.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// The ciphertext matched its manifest and still failed authentication,
    /// which a wrong PIN is the likeliest cause of.
    WrongPin,
    /// The download, its manifest or its contents are not a valid package.
    Invalid(TaskError),
}

impl From<TaskError> for OpenError {
    fn from(error: TaskError) -> Self {
        Self::Invalid(error)
    }
}

fn compact(value: &Value) -> Vec<u8> {
    // `serde_json` writes object keys in insertion order and arrays as given,
    // without whitespace, which for these arrays of strings and integers is the
    // compact encoding the Python side produces.
    serde_json::to_vec(value).expect("JSON values serialize")
}

/// Six ASCII digits, leading zeros kept.
pub fn valid_pin(pin: &str) -> bool {
    pin.len() == default_number("pinDigits") as usize && pin.bytes().all(|b| b.is_ascii_digit())
}

/// PBKDF2-HMAC-SHA256 of the PIN, salted with the set, version and the
/// manifest's random salt. The iteration count belongs to the format, never
/// to the manifest, so a website cannot set how much work this does.
fn derive_key(pin: &str, set_id: &str, version: u32, salt: &str) -> [u8; 32] {
    let iterations =
        NonZeroU32::new(default_number("kdfIterations") as u32).expect("positive KDF iterations");
    let salt = compact(&serde_json::json!([
        "codetrial-task-kdf-v1",
        set_id,
        version,
        salt
    ]));
    let mut key = [0; 32];
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        iterations,
        &salt,
        pin.as_bytes(),
        &mut key,
    );
    key
}

/// The numeric rules by name, then `closesAt` and `rulesNote`.
type CheckedRules = (BTreeMap<String, u64>, Option<String>, Option<String>);

fn checked_rules(rules: Rules) -> Result<CheckedRules, TaskError> {
    let duration =
        u64::from(crate::config::MIN_DURATION_MIN)..=u64::from(crate::config::MAX_DURATION_MIN);
    if !duration.contains(&rules.duration_min)
        || rules.max_hint_rungs > default_number("maxHintRungs")
        || !(5..=60).contains(&rules.look_away_seconds)
    {
        return Err(invalid("set rules outside bounds"));
    }
    if let Some(close) = &rules.closes_at {
        parse_close(close)?;
    }
    if rules
        .rules_note
        .as_deref()
        .is_some_and(|note| note.trim().is_empty() || note.len() > MAX_RULES_NOTE_BYTES)
    {
        return Err(invalid("invalid rules note"));
    }
    Ok((
        BTreeMap::from([
            ("durationMin".to_owned(), rules.duration_min),
            ("maxHintRungs".to_owned(), rules.max_hint_rungs),
            ("lookAwaySeconds".to_owned(), rules.look_away_seconds),
        ]),
        rules.closes_at,
        rules.rules_note,
    ))
}

/// Opens one downloaded set version with `pin`. The manifest's hash and size
/// are checked before any key is derived, so a corrupt download is `Invalid`
/// and only a ciphertext that matches its manifest can be a wrong PIN.
pub fn open_package(
    set_id: &str,
    version: u32,
    manifest_bytes: &[u8],
    // Owned, so it is decrypted where it lies rather than copied first.
    mut ciphertext: Vec<u8>,
    pin: &str,
) -> Result<LoadedSet, OpenError> {
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(invalid("manifest exceeds size limit").into());
    }
    let manifest: Manifest =
        serde_json::from_slice(manifest_bytes).map_err(|_| invalid("invalid manifest schema"))?;
    let salt_bytes = base64::engine::general_purpose::STANDARD
        .decode(&manifest.salt)
        .map_err(|_| invalid("invalid manifest salt"))?;
    if manifest.package_version != 1
        || manifest.set_id != set_id
        || manifest.set_version != version
        || salt_bytes.len() != 16
        || manifest.ciphertext_bytes != ciphertext.len()
        || !(28..=default_number("setBytes") as usize + 28).contains(&ciphertext.len())
        || manifest.ciphertext_sha256 != crate::sha256_hex(&[&ciphertext])
    {
        return Err(invalid("invalid manifest or ciphertext").into());
    }
    if !valid_pin(pin) {
        return Err(OpenError::WrongPin);
    }
    let key = derive_key(pin, set_id, version, &manifest.salt);
    let nonce = aead::Nonce::try_assume_unique_for_key(&ciphertext[..12])
        .map_err(|_| invalid("invalid nonce"))?;
    // `Nonce` copies its bytes, so the ciphertext is free to be overwritten.
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_256_GCM, &key).expect("32-byte AES-256 key"),
    );
    let aad = compact(&serde_json::json!([
        "codetrial-task-set-v1",
        set_id,
        version
    ]));
    let plaintext = key
        .open_within(nonce, aead::Aad::from(aad), &mut ciphertext, 12..)
        .map_err(|_| OpenError::WrongPin)?;
    Ok(decode(set_id, version, plaintext)?)
}

/// A decrypted package checked and posed: the authenticated metadata must
/// name the set it was opened for, the rules must be in bounds, and every
/// record must validate. Apart from `open_package` so these checks are tested
/// without paying a key derivation each time.
pub(crate) fn decode(set_id: &str, version: u32, plaintext: &[u8]) -> Result<LoadedSet, TaskError> {
    let package: Package =
        serde_json::from_slice(plaintext).map_err(|_| invalid("invalid package schema"))?;
    if package.package_version != 1
        || package.set_id != set_id
        || package.set_version != version
        || package.problems.is_empty()
    {
        return Err(invalid("invalid authenticated package metadata"));
    }
    let (rules, closes_at, rules_note) = checked_rules(package.rules)?;
    let mut records = BTreeMap::new();
    for problem in &package.problems {
        let id = problem["id"]
            .as_str()
            .ok_or_else(|| invalid("missing problem id"))?;
        let judge = package
            .judges
            .get(id)
            .ok_or_else(|| invalid("missing judge"))?;
        let variant = package
            .variants
            .get(id)
            .ok_or_else(|| invalid("missing variant"))?;
        let sidecar = package.sidecars.get(id);
        let row = serde_json::json!({"problem": problem, "judge": judge, "variant": variant, "sidecar": sidecar});
        if serde_json::to_vec(&row).unwrap().len() > default_number("taskBytes") as usize {
            return Err(invalid("plaintext task exceeds limit"));
        }
        if records
            .insert(
                id.to_owned(),
                Arc::new(TaskRecord::from_bank(problem, judge, variant, sidecar)?),
            )
            .is_some()
        {
            return Err(invalid("duplicate task id"));
        }
    }
    let ids: HashSet<_> = records.keys().collect();
    if package.judges.keys().collect::<HashSet<_>>() != ids
        || package.variants.keys().collect::<HashSet<_>>() != ids
        || package.sidecars.keys().any(|id| !records.contains_key(id))
    {
        return Err(invalid("orphan bank record"));
    }
    Ok(LoadedSet {
        config: SetConfig {
            id: set_id.to_owned(),
            version,
            closes_at,
            rules_note,
            rules,
        },
        records,
    })
}

/// The instructor site an assignment names: an HTTPS URL without
/// credentials, query or fragment, normalized to end in `/` so `sets/` joins
/// under its path.
pub fn site_base(text: &str) -> Result<reqwest::Url, TaskError> {
    let mut url = reqwest::Url::parse(text).map_err(|_| invalid("invalid site URL"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "site must be an HTTPS URL without credentials or query",
        ));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

/// A downloaded set version, not yet opened.
pub struct Download {
    pub manifest: Vec<u8>,
    pub ciphertext: Vec<u8>,
    /// The site's clock, from its `Date` header, in epoch seconds, and when
    /// that response arrived: time spent downloading and decrypting after it
    /// still counts toward a close date.
    pub site_time: Option<(u64, tokio::time::Instant)>,
}

/// Fetches one set version's manifest and ciphertext under the assignment's
/// site, bounded in size and time, refusing redirects. `Assignment::new` has
/// checked the id and version that name the path, and the site is from
/// `site_base`, except in tests that serve packages over local HTTP.
pub async fn download(assignment: &Assignment) -> Result<Download, TaskError> {
    let Assignment {
        site,
        set_id,
        version,
    } = assignment;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(default_number("fetchSeconds")))
        .build()
        .map_err(|_| invalid("task HTTP client failed"))?;
    let prefix = format!("sets/{set_id}/{version}/");
    let url = |name: &str| {
        site.join(&format!("{prefix}{name}"))
            .map_err(|_| invalid("invalid package URL"))
    };
    // Independent until both have arrived, so fetched together.
    let ((manifest, site_time), (ciphertext, _)) = tokio::try_join!(
        fetch(&client, url("manifest.json")?, MAX_MANIFEST_BYTES),
        fetch(
            &client,
            url("tasks.enc")?,
            default_number("setBytes") as usize + 28,
        ),
    )?;
    Ok(Download {
        manifest,
        ciphertext,
        site_time,
    })
}

async fn fetch(
    client: &reqwest::Client,
    url: reqwest::Url,
    limit: usize,
) -> Result<(Vec<u8>, Option<(u64, tokio::time::Instant)>), TaskError> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| invalid("package fetch failed"))?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|size| size > limit as u64)
    {
        return Err(invalid("package response refused"));
    }
    let site_time = response
        .headers()
        .get(reqwest::header::DATE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| chrono::DateTime::parse_from_rfc2822(value).ok())
        .and_then(|time| u64::try_from(time.timestamp()).ok())
        .map(|at| (at, tokio::time::Instant::now()));
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| invalid("package fetch interrupted"))?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(invalid("package response exceeds limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((bytes, site_time))
}

#[cfg(test)]
#[path = "../../tests/unit/tasks/package.rs"]
mod tests;

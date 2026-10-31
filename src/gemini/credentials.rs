use std::collections::HashMap;
use std::io;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::config::AgentConfig;

pub(crate) const QUOTA_COOLDOWN: Duration = Duration::from_secs(60);

// As long as the web pool trusts a refused LiveKit credential verdict. A
// refusal that was about the project rather than the key, and so marked every
// key, clears on its own; a dead key costs a room one quick refusal a minute.
const INVALID_COOLDOWN: Duration = Duration::from_secs(60);
static COOLDOWNS: OnceLock<Mutex<HashMap<String, Cooldown>>> = OnceLock::new();

/// The reasons Google gives for refusing a key, and for refusing it on quota.
/// A response carries them as `details[].reason`, a Live close frame only
/// inside its reason text, so both classifiers read these lists.
const INVALID_REASONS: &[&str] = &[
    "API_KEY_INVALID",
    "API_KEY_EXPIRED",
    "API_KEY_SERVICE_BLOCKED",
    "API_KEY_IP_ADDRESS_BLOCKED",
    "API_KEY_HTTP_REFERRER_BLOCKED",
    "API_KEY_ANDROID_APP_BLOCKED",
    "API_KEY_IOS_APP_BLOCKED",
];
const QUOTA_REASONS: &[&str] = &["QUOTA_EXCEEDED", "RATE_LIMIT_EXCEEDED"];

/// The same three verdicts in the words of a message or close reason, read
/// billing first: a depleted prepayment can arrive worded as an exhausted
/// resource, and waiting out a quota cooldown would not bring it back. Never
/// the bare word "billing": Google's ordinary rate-limit text ends by asking
/// the reader to "check your plan and billing details", and a rate limit is
/// exactly the failure that waiting does fix.
const BILLING_PHRASES: &[&str] = &[
    "prepayment credits",
    "credits are depleted",
    "requires billing",
    "billing to be enabled",
    "billing is not enabled",
    "billing has not been enabled",
    "billing account",
    "billing_disabled",
];
const QUOTA_PHRASES: &[&str] = &[
    "resource_exhausted",
    "quota exceeded",
    "exceeded your current quota",
    "quota exhausted",
];
const INVALID_PHRASES: &[&str] = &[
    "api key not valid",
    "api key was reported as leaked",
    "api key has expired",
    "api key is disabled",
];

#[derive(Default)]
struct Cooldown {
    live: Option<Instant>,
    report: Option<Instant>,

    // Rejections cannot recover just by waiting. Each marker expires no later
    // than its surface deadline, so pruning needs only those deadlines.
    live_rejected: Option<Instant>,
    report_rejected: Option<Instant>,

    // Read only to say why a rotation ran dry, in the same way.
    billing: Option<Instant>,
}

// Pruning and selection share the same expiry boundary.
fn cooling_down(until: Option<Instant>, now: Instant) -> bool {
    until.is_some_and(|until| until > now)
}

// A later failure must not shorten a deadline already set.
fn extend(slot: &mut Option<Instant>, until: Instant) {
    *slot = Some(slot.map_or(until, |old| old.max(until)));
}

#[derive(Clone, Copy)]
pub(super) enum ApiSurface {
    Live,
    Report,
}

#[derive(Default)]
struct Selection {
    live: usize,
    report: Option<usize>,
}

/// Selection belongs to an interview; known failures belong to the process.
/// Neither this type nor the shared map is printable.
///
/// A sole key is never taken out of rotation. Marking it would only turn one
/// failure into a refusal of every room this process is handed, or of every
/// restart inside a cooldown, with nothing to move to instead. It keeps the
/// retry budgets it had before failover existed, and a rejection is reported
/// by the caller that saw it.
pub struct GeminiKeys {
    keys: Vec<String>,
    current: Mutex<Selection>,
}

impl GeminiKeys {
    pub fn from_config(config: &AgentConfig) -> Self {
        Self::new(config.google_api_keys.clone())
    }

    pub fn single(key: &str) -> Self {
        Self::new(if key.is_empty() {
            Vec::new()
        } else {
            vec![key.to_string()]
        })
    }

    fn new(keys: Vec<String>) -> Self {
        Self {
            keys,
            current: Mutex::new(Selection::default()),
        }
    }

    pub(crate) fn has_backups(&self) -> bool {
        self.keys.len() > 1
    }

    pub(super) fn count(&self) -> usize {
        self.keys.len()
    }

    pub(super) fn redaction_keys(&self) -> Vec<String> {
        self.keys.clone()
    }

    pub fn select(&self) -> Result<String, io::Error> {
        self.select_for(ApiSurface::Live)
    }

    pub(crate) fn select_report(&self) -> Result<String, io::Error> {
        self.select_for(ApiSurface::Report)
    }

    /// A rotation whose every key is out on report quota, for callers outside
    /// this module that decide from the rotation's answer.
    #[cfg(test)]
    pub(crate) fn report_quota_exhausted(keys: &[&str]) -> Self {
        let rotation = Self::new(keys.iter().map(|key| key.to_string()).collect());
        for key in keys {
            rotation.failed(key, CredentialFailure::Quota, ApiSurface::Report);
        }
        rotation
    }

    #[cfg(test)]
    pub(super) fn select_report_at(&self, now: Instant) -> Result<String, io::Error> {
        // A simulated future must not expire another test's credentials.
        self.select_for_at(ApiSurface::Report, now, false)
    }

    fn select_for(&self, surface: ApiSurface) -> Result<String, io::Error> {
        self.select_for_at(surface, Instant::now(), true)
    }

    fn select_for_at(
        &self,
        surface: ApiSurface,
        now: Instant,
        prune: bool,
    ) -> Result<String, io::Error> {
        // Short of the shared map as well, which another interview's list
        // holding the same key string would otherwise write for it.
        if !self.has_backups() {
            return self
                .keys
                .first()
                .cloned()
                .ok_or_else(|| exhausted(None, false));
        }
        let mut cooldowns = COOLDOWNS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if prune {
            cooldowns.retain(|_, cooldown| {
                cooling_down(cooldown.live, now) || cooling_down(cooldown.report, now)
            });
        }
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let start = match surface {
            ApiSurface::Live => current.live,
            ApiSurface::Report => current.report.unwrap_or(current.live),
        };
        let index = std::iter::once(start)
            .chain(start.saturating_add(1)..self.keys.len())
            .chain(0..start)
            .find(|&index| {
                self.keys.get(index).is_some_and(|key| {
                    !cooldowns.get(key).is_some_and(|cooldown| {
                        cooling_down(
                            match surface {
                                ApiSurface::Live => cooldown.live,
                                ApiSurface::Report => cooldown.report,
                            },
                            now,
                        )
                    })
                })
            })
            .ok_or_else(|| {
                // A quota deadline can recover either surface. A rejected
                // credential must not masquerade as a temporary rate limit.
                let retry_at = match surface {
                    ApiSurface::Live => self
                        .keys
                        .iter()
                        .filter_map(|key| cooldowns.get(key))
                        .filter(|cooldown| !cooling_down(cooldown.live_rejected, now))
                        .filter_map(|cooldown| cooldown.live)
                        .min(),
                    ApiSurface::Report => self
                        .keys
                        .iter()
                        .filter_map(|key| cooldowns.get(key))
                        .filter(|cooldown| !cooling_down(cooldown.report_rejected, now))
                        .filter_map(|cooldown| cooldown.report)
                        .min(),
                };
                let billing = self.keys.iter().any(|key| {
                    cooldowns
                        .get(key)
                        .is_some_and(|cooldown| cooling_down(cooldown.billing, now))
                });
                exhausted(retry_at, billing)
            })?;
        match surface {
            ApiSurface::Live => current.live = index,
            ApiSurface::Report => current.report = Some(index),
        }
        Ok(self.keys[index].clone())
    }

    pub(super) fn failed(&self, key: &str, failure: CredentialFailure, surface: ApiSurface) {
        if !self.has_backups() {
            return;
        }
        let mut cooldowns = COOLDOWNS
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let entry = cooldowns.entry(key.to_string()).or_default();
        let now = Instant::now();
        let rejected = now + INVALID_COOLDOWN;
        match failure {
            // Credit belongs to the project behind the key, so both surfaces
            // are out until someone pays; another project's key may not be.
            CredentialFailure::Invalid | CredentialFailure::Billing => {
                extend(&mut entry.live, rejected);
                extend(&mut entry.report, rejected);
                extend(&mut entry.live_rejected, rejected);
                extend(&mut entry.report_rejected, rejected);
                if failure == CredentialFailure::Billing {
                    extend(&mut entry.billing, rejected);
                }
            }
            CredentialFailure::Refused => match surface {
                ApiSurface::Live => {
                    extend(&mut entry.live, rejected);
                    extend(&mut entry.live_rejected, rejected);
                }
                ApiSurface::Report => {
                    extend(&mut entry.report, rejected);
                    extend(&mut entry.report_rejected, rejected);
                }
            },
            CredentialFailure::Quota => extend(
                match surface {
                    ApiSurface::Live => &mut entry.live,
                    ApiSurface::Report => &mut entry.report,
                },
                now + QUOTA_COOLDOWN,
            ),
        }
    }

    pub fn redact(&self, text: &str) -> String {
        super::redact_api_keys(text, &self.keys)
    }
}

/// No key is available, and when one will be if waiting is worth it.
#[derive(Debug)]
struct Exhausted {
    retry_at: Option<Instant>,
    /// A key is out because its project cannot pay, which is the reason an
    /// interview names when the rotation it ran dry was billing's doing.
    billing: bool,
}

impl std::fmt::Display for Exhausted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Gemini credentials exhausted: all configured keys are unavailable")
    }
}

impl std::error::Error for Exhausted {}

fn exhausted(retry_at: Option<Instant>, billing: bool) -> io::Error {
    io::Error::other(Exhausted { retry_at, billing })
}

#[cfg(test)]
pub(super) fn exhausted_for_test(retry_at: Instant) -> io::Error {
    exhausted(Some(retry_at), false)
}

fn as_exhausted<'a>(error: &'a (dyn std::error::Error + 'static)) -> Option<&'a Exhausted> {
    error
        .downcast_ref::<io::Error>()?
        .get_ref()?
        .downcast_ref::<Exhausted>()
}

/// Whether `error` is a rotation emptied with a key out for billing.
pub(super) fn exhausted_by_billing(error: &(dyn std::error::Error + 'static)) -> bool {
    as_exhausted(error).is_some_and(|exhausted| exhausted.billing)
}

/// When an exhausted rotation has a key back from its quota cooldown.
pub(crate) fn exhausted_until(error: &(dyn std::error::Error + 'static)) -> Option<Instant> {
    as_exhausted(error)?.retry_at
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CredentialFailure {
    Quota,
    Invalid,
    /// A 403 that says nothing about why. It may be about what this surface
    /// asked for rather than the key, such as a report model the project cannot
    /// use, so it takes the key off that surface alone.
    Refused,
    /// The project cannot pay: a 402, or a close reason saying the prepaid
    /// credit is depleted. Unlike a rate limit it does not clear by waiting,
    /// so it is retried only on another key.
    Billing,
}

impl CredentialFailure {
    /// Whether only another key can get past this, the same key failing the
    /// same way however long it waits. Retry policy on every surface reads
    /// this, so a new kind of failure is classified once.
    pub(super) fn needs_other_key(self) -> bool {
        match self {
            Self::Quota => false,
            Self::Invalid | Self::Refused | Self::Billing => true,
        }
    }
}

#[derive(Debug)]
pub(super) struct ApiFailure {
    pub status: u16,
    pub credential: Option<CredentialFailure>,
    pub detail: Option<String>,
}

impl std::fmt::Display for ApiFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(detail) = &self.detail {
            return formatter.write_str(detail);
        }
        match self.credential {
            Some(CredentialFailure::Quota) => return formatter.write_str("Gemini quota exhausted"),
            Some(CredentialFailure::Billing) => {
                return formatter.write_str("Gemini billing or prepaid credit exhausted");
            }
            Some(CredentialFailure::Invalid | CredentialFailure::Refused) => {
                return formatter.write_str("Gemini credential rejected");
            }
            None => {}
        }
        if self.status == 0 {
            return formatter.write_str("Gemini connection or setup failed");
        }

        // Not "rejected": a 5xx is Google saying it could not serve anyone just
        // then, and the report note that carries this is the one place the
        // candidate learns that trying later is the whole remedy.
        if self.status >= 500 {
            return write!(formatter, "Gemini unavailable (status={})", self.status);
        }
        write!(
            formatter,
            "Gemini request rejected (status={})",
            self.status
        )
    }
}

impl std::error::Error for ApiFailure {}

impl ApiFailure {
    pub(super) fn from_response(status: u16, body: &Value) -> Self {
        // Billing first, as `failure_from_reason` reads it: a depleted project
        // can answer 429 or RESOURCE_EXHAUSTED, and a quota cooldown on the
        // same key would never bring it back.
        let billing = matches!(status, 400 | 403 | 429)
            && failure_from_reason(body["error"]["message"].as_str().unwrap_or_default())
                == Some(CredentialFailure::Billing);
        let credential = if status == 402 || billing {
            Some(CredentialFailure::Billing)
        } else if status == 429 {
            Some(CredentialFailure::Quota)
        } else if status == 401 {
            Some(CredentialFailure::Invalid)
        } else if matches!(status, 400 | 403) {
            let error = &body["error"];
            let gives = |reasons: &[&str]| {
                error["details"].as_array().is_some_and(|details| {
                    details.iter().any(|detail| {
                        detail["reason"]
                            .as_str()
                            .is_some_and(|reason| reasons.contains(&reason))
                    })
                })
            };
            if gives(INVALID_REASONS) {
                Some(CredentialFailure::Invalid)
            } else if error["status"].as_str() == Some("RESOURCE_EXHAUSTED") || gives(QUOTA_REASONS)
            {
                Some(CredentialFailure::Quota)
            } else if let Some(failure) =
                failure_from_reason(error["message"].as_str().unwrap_or_default())
            {
                Some(failure)
            } else if status == 403 {
                // A 403 nothing above explains is still a refusal of this key,
                // and usually the one with no body to explain it: a WebSocket
                // handshake rejection carries none. Retrying the same key
                // spends the budget while the backups sit idle. The mark
                // expires, so a refusal that was about the project rather than
                // the key does not empty the rotation for good. A 400 stays
                // reason-based, because a malformed request would otherwise be
                // charged to every key in turn.
                Some(CredentialFailure::Refused)
            } else {
                None
            }
        } else {
            None
        };

        // Google names the fault in the body, "The model is overloaded" being
        // the usual one, and a bare status left the reader to guess between an
        // overload and an outage. Bounded and flattened to one line, since it
        // is outside text on its way into a log line and a report card. Only a
        // message with something in it, or the card reads "(status=503):."
        // where the status-only wording would have done.
        let detail = (status >= 500)
            .then(|| body["error"]["message"].as_str())
            .flatten()
            .map(|message| {
                message
                    .trim()
                    .chars()
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .take(200)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .filter(|message| !message.is_empty())
            .map(|message| format!("Gemini unavailable (status={status}): {message}"));
        Self {
            status,
            credential,
            detail,
        }
    }
}

pub(super) fn failure_from_reason(reason: &str) -> Option<CredentialFailure> {
    let code = reason.to_ascii_uppercase();
    let names = |reasons: &[&str]| reasons.iter().any(|reason| code.contains(reason));
    let reason = reason.to_ascii_lowercase();
    let says = |phrases: &[&str]| phrases.iter().any(|phrase| reason.contains(phrase));
    if says(BILLING_PHRASES) {
        Some(CredentialFailure::Billing)
    } else if says(QUOTA_PHRASES) || names(QUOTA_REASONS) {
        Some(CredentialFailure::Quota)
    } else if says(INVALID_PHRASES) || names(INVALID_REASONS) {
        Some(CredentialFailure::Invalid)
    } else {
        None
    }
}

pub(super) fn credential_failure(
    error: &(dyn std::error::Error + 'static),
) -> Option<CredentialFailure> {
    if let Some(error) = error.downcast_ref::<ApiFailure>() {
        return error.credential;
    }
    if let Some(tokio_tungstenite::tungstenite::Error::Http(response)) =
        error.downcast_ref::<tokio_tungstenite::tungstenite::Error>()
    {
        let body = response
            .body()
            .as_deref()
            .and_then(|body| serde_json::from_slice(body).ok())
            .unwrap_or(Value::Null);
        return ApiFailure::from_response(response.status().as_u16(), &body).credential;
    }
    None
}

#[cfg(test)]
#[path = "../../tests/unit/gemini/credentials.rs"]
mod tests;

//! Operator bounds for two-person sessions, independent of Gemini settings.
use super::{ConfigError, recording_number};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerConfig {
    pub max_sessions: usize,
    pub round_minutes: u32,
    pub hard_max_minutes: u32,
    pub invite_ttl_days: u32,
    pub reconnect_grace_seconds: u64,
}

impl Default for PeerConfig {
    fn default() -> Self {
        Self {
            max_sessions: 8,
            round_minutes: 25,
            hard_max_minutes: 90,
            invite_ttl_days: 7,
            reconnect_grace_seconds: 120,
        }
    }
}

pub fn load_peer(values: &BTreeMap<String, String>) -> Result<PeerConfig, ConfigError> {
    let mut invalid_entries = Vec::new();
    let defaults = PeerConfig::default();
    let mut positive = |key, default| {
        let value = recording_number(values, key, default, &mut invalid_entries);
        if value == 0 {
            invalid_entries.push(format!("{key} must be positive"));
        }
        value
    };
    let config = PeerConfig {
        max_sessions: usize::try_from(positive("CODETRIAL_PEER_MAX_SESSIONS", 8))
            .expect("u32 fits usize on supported platforms"),
        round_minutes: positive("CODETRIAL_PEER_ROUND_MINUTES", defaults.round_minutes),
        hard_max_minutes: positive("CODETRIAL_PEER_HARD_MAX_MINUTES", defaults.hard_max_minutes),
        invite_ttl_days: positive("CODETRIAL_PEER_INVITE_TTL_DAYS", defaults.invite_ttl_days)
            .min(14),
        reconnect_grace_seconds: u64::from(positive("CODETRIAL_PEER_RECONNECT_GRACE_SECONDS", 120)),
    };
    if config.round_minutes.saturating_mul(2) > config.hard_max_minutes {
        invalid_entries.push(
            "CODETRIAL_PEER_HARD_MAX_MINUTES must cover both rounds of CODETRIAL_PEER_ROUND_MINUTES"
                .into(),
        );
    }
    if invalid_entries.is_empty() {
        Ok(config)
    } else {
        Err(ConfigError {
            missing_keys: Vec::new(),
            invalid_entries,
        })
    }
}

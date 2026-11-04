//! Startup validation for the opt-in reciprocal interview service.
use codetrial::config::{PeerObserverConfig, load_peer_observer};
use std::collections::BTreeMap;

pub(super) fn peer_needs_oauth(peer: bool, oauth: bool) -> Result<(), String> {
    if peer && !oauth {
        return Err(
            "CODETRIAL_PEER_ENABLED=true needs GITHUB_CLIENT_ID and GITHUB_CLIENT_SECRET".into(),
        );
    }
    Ok(())
}

pub(super) fn observer_config(
    values: &BTreeMap<String, String>,
    enabled: bool,
) -> Result<Option<PeerObserverConfig>, String> {
    if !enabled {
        return Ok(None);
    }
    let config = load_peer_observer(values)?;
    if config.is_some()
        && codetrial::config::load_peer(values)
            .map_err(|error| error.to_string())?
            .is_some_and(|peer| peer.hard_max_minutes > 120)
    {
        return Err("CODETRIAL_PEER_HARD_MAX_MINUTES must be at most 120 because the silent observer captures at most two hours".into());
    }
    if config.is_none() {
        return Err("CODETRIAL_PEER_ENABLED=true needs GOOGLE_API_KEY or GOOGLE_API_KEYS for the silent observer and post-session advice".into());
    }
    Ok(config)
}

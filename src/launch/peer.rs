//! Startup rules for reciprocal practice, offered once its prerequisites exist.
use crate::config::{
    MAX_CAPTURE_SECONDS, PeerConfig, PeerObserverConfig, load_peer, load_peer_observer,
};
use std::collections::BTreeMap;

pub type PeerServices = Option<(PeerConfig, PeerObserverConfig)>;

/// Reciprocal practice needs GitHub sign-in for member identity and the Google
/// keys for the silent observer. Without either it is simply not offered.
pub fn peer_services(
    values: &BTreeMap<String, String>,
    oauth: bool,
) -> Result<PeerServices, String> {
    let peer = load_peer(values).map_err(|error| error.to_string())?;
    let observer = load_peer_observer(values)?;
    let (true, Some(observer)) = (oauth, observer) else {
        return Ok(None);
    };
    if u64::from(peer.hard_max_minutes) > MAX_CAPTURE_SECONDS / 60 {
        return Err("CODETRIAL_PEER_HARD_MAX_MINUTES must be at most 120 because the silent observer captures at most two hours".into());
    }
    Ok(Some((peer, observer)))
}

//! Peer observation uses text output only, without a Live voice session.
use super::{GENERATION_SEED, gemini_generate_content_url, side_call_once};
use crate::config::{
    ensure_private_directory, google_api_keys, peer_audio_storage_is_private, present,
    report_model_or_default,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct PeerObserverConfig {
    pub(crate) keys: Arc<crate::gemini::GeminiKeys>,
    pub model: String,
    pub audio_dir: Option<PathBuf>,
    pub(crate) endpoint: Option<String>,
}

// A cloned server configuration shares its mutable credential rotation state.
impl PartialEq for PeerObserverConfig {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.keys, &other.keys)
            && self.model == other.model
            && self.audio_dir == other.audio_dir
            && self.endpoint == other.endpoint
    }
}
impl Eq for PeerObserverConfig {}

impl std::fmt::Debug for PeerObserverConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PeerObserverConfig")
            .field("keys", &"[redacted]")
            .field("model", &self.model)
            .field("audio_available", &self.audio_dir.is_some())
            .finish_non_exhaustive()
    }
}

pub fn load_peer_observer(
    values: &BTreeMap<String, String>,
) -> Result<Option<PeerObserverConfig>, String> {
    let keys = google_api_keys(values)?;
    if keys.is_empty() {
        return Ok(None);
    }
    let audio_dir = present(values, "CODETRIAL_PEER_AUDIO_DIR").map(PathBuf::from);
    if let Some(dir) = &audio_dir {
        ensure_private_directory(dir).map_err(|error| {
            format!("CODETRIAL_PEER_AUDIO_DIR must be a writable private directory: {error}")
        })?;
    }
    Ok(Some(PeerObserverConfig {
        keys: Arc::new(super::GeminiKeys::new(keys)),
        model: report_model_or_default(values),
        audio_dir,
        endpoint: None,
    }))
}

impl PeerObserverConfig {
    pub fn validate_audio_storage(&self, web_dir: &std::path::Path) -> Result<(), String> {
        if self
            .audio_dir
            .as_ref()
            .is_some_and(|dir| !peer_audio_storage_is_private(dir, web_dir))
        {
            return Err("CODETRIAL_PEER_AUDIO_DIR must be outside CODETRIAL_WEB_DIR and its symlink targets".into());
        }
        Ok(())
    }
}

static HTTP_CALLS: Semaphore = Semaphore::const_new(4);

pub(crate) async fn generate_peer_content(
    config: &PeerObserverConfig,
    system: &str,
    parts: Vec<Value>,
    scope: &str,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let _permit = tokio::time::timeout(Duration::from_secs(12), HTTP_CALLS.acquire()).await??;
    let request = json!({
        "systemInstruction":{"parts":[{"text":system}]},
        "contents":[{"role":"user","parts":parts}],
        "generationConfig":{"responseModalities":["TEXT"],"responseMimeType":"application/json",
            "maxOutputTokens":4096,"temperature":0.0,"seed":GENERATION_SEED,
            "thinkingConfig":{"thinkingBudget":0}},
    });
    let url = config
        .endpoint
        .clone()
        .unwrap_or_else(|| gemini_generate_content_url(&config.model));

    // A rate limit does not cool the key: peer rooms share the report keys with
    // interviews, and the final report of a concurrent one must not find them
    // all cooled, as a phase judgment's rate limit does not either.
    let text = side_call_once(&config.keys, &url, &request, "peer-observer", scope, false).await?;
    Ok(serde_json::from_str(&text)?)
}

#[cfg(test)]
#[path = "../../tests/unit/gemini/peer.rs"]
mod tests;

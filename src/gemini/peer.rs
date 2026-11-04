//! Peer observation uses text output only, without a Live voice session.
use super::{GENERATION_SEED, gemini_generate_content_url, side_call_once};
use crate::config::PeerObserverConfig;
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::Semaphore;

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
    let text = side_call_once(&config.keys, &url, &request, "peer-observer", scope, true).await?;
    Ok(serde_json::from_str(&text)?)
}

#[cfg(test)]
#[path = "../../tests/unit/gemini/peer.rs"]
mod tests;

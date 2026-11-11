//! The acknowledgement clip: the speech model's answer, checked to be raw PCM
//! at the output track's rate and a plausible length before it is played.

use std::io;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;

/// The PCM in a speech response, refusing anything but raw 16-bit samples at
/// the output track's rate: a container or a codec would play as noise.
pub(super) fn tool_ack_pcm(
    response: &Value,
    sample_rate: u32,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let data = &response["candidates"][0]["content"]["parts"][0]["inlineData"];

    // Media types are case-insensitive, and the speech model answers in lower
    // case: "audio/l16; rate=24000; channels=1".
    let mime = data["mimeType"].as_str().unwrap_or_default();
    let lower = mime.to_ascii_lowercase();
    let media_type = lower.split(';').next().unwrap_or_default().trim();
    let raw = matches!(media_type, "audio/l16" | "audio/pcm");
    let rate = format!("rate={sample_rate}");
    if !raw || !lower.split(';').any(|param| param.trim() == rate) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("acknowledgement audio is {mime:?}, not {sample_rate} Hz PCM"),
        )
        .into());
    }
    let encoded = data["data"].as_str().unwrap_or_default();
    let pcm = STANDARD.decode(encoded)?;
    let longest = sample_rate as usize * 2 * TOOL_ACK_MAX_SECONDS;
    if pcm.is_empty() || pcm.len() > longest {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "acknowledgement audio is {} bytes, not 1 to {longest}",
                pcm.len()
            ),
        )
        .into());
    }
    Ok(pcm)
}

/// Far longer than "Mm, okay." takes to say; a clip past it is not one.
const TOOL_ACK_MAX_SECONDS: usize = 10;

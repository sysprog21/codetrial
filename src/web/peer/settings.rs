//! Defaults apply only to omitted fields; malformed supplied values are errors.
use crate::accounts::peer::{PeerResult, Settings};
use crate::config::PeerConfig;
use serde_json::Value;

pub(super) fn parse(body: &Value, config: &PeerConfig) -> PeerResult<Settings> {
    let fields = body.as_object().ok_or("invalid_settings")?;
    let text = |key, default| match fields.get(key) {
        None => Ok(default),
        Some(value) => value.as_str().ok_or("invalid_settings"),
    };
    let number = |key, default| match fields.get(key) {
        None => Ok(default),
        Some(value) => value.as_u64().ok_or("invalid_settings"),
    };
    let round_minutes = u32::try_from(number("roundMinutes", u64::from(config.round_minutes))?)
        .map_err(|_| "invalid_settings")?;
    let interviewer_first =
        u8::try_from(number("interviewerFirst", 0)?).map_err(|_| "invalid_settings")?;
    if interviewer_first > 1 {
        return Err("invalid_settings");
    }
    let flag = |key| match fields.get(key) {
        None => Ok(false),
        Some(value) => value.as_bool().ok_or("invalid_settings"),
    };
    Ok(Settings {
        topic: text("topic", "")?.to_owned(),
        difficulty: text("difficulty", "easy")?.to_owned(),
        language: text("language", "python")?.to_owned(),
        round_minutes,
        interviewer_first: usize::from(interviewer_first),
        save_transcript: flag("saveTranscript")?,
        save_audio: flag("saveAudio")?,
    })
}

#[cfg(test)]
#[path = "../../../tests/unit/web/peer/settings.rs"]
mod tests;

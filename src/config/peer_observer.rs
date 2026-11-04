//! Keys for the silent peer observer are never printed in configuration logs.
use super::{google_api_keys, present, report_model_or_default};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// The longest stretch the silent observer captures, shared by the startup
/// deadline check and the runtime bound so the two cannot drift apart.
pub const MAX_CAPTURE_SECONDS: u64 = 2 * 60 * 60;

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
        crate::peer_audio::ensure_private_directory(dir).map_err(|_| {
            "CODETRIAL_PEER_AUDIO_DIR must be a writable private directory".to_owned()
        })?;
    }
    Ok(Some(PeerObserverConfig {
        keys: Arc::new(crate::gemini::GeminiKeys::new(keys)),
        model: report_model_or_default(values),
        audio_dir,
        endpoint: None,
    }))
}

/// The static-file server runs as the same user as the private audio writer,
/// so filesystem mode bits cannot protect a directory beneath its web root.
pub fn peer_audio_storage_is_private(
    audio_dir: &std::path::Path,
    web_dir: &std::path::Path,
) -> bool {
    let Ok(audio) = audio_dir.canonicalize() else {
        return false;
    };
    match web_dir.canonicalize() {
        Ok(web) => !audio.starts_with(&web) && !symlink_reaches(&web, &audio),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// A symlink under the web root whose target is the audio directory, or one of
/// its parents, would let the static server read the raw recordings through it.
fn symlink_reaches(dir: &std::path::Path, audio: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        if kind.is_symlink() {
            return path
                .canonicalize()
                .is_ok_and(|target| audio.starts_with(&target) || target.starts_with(audio));
        }
        kind.is_dir() && symlink_reaches(&path, audio)
    })
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

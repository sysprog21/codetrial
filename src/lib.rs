pub mod accounts;
pub mod agent;
pub mod config;
pub mod delivery;
pub mod dispatch;
pub mod gemini;
pub mod livekit;
pub mod recording;
pub mod runtime;
pub mod token;
pub mod web;

/// Lives here rather than in one of the modules because `accounts` stamps rows
/// with it and `web` stamps tokens with it, and neither should have to depend
/// on the other for a clock.
pub fn current_epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The folder the executable itself sits in, which is where anything
/// `codetrial` writes or looks for without being told a path belongs. A
/// released binary is unpacked into a folder of its own and everything it
/// needs ends up there, whatever directory it was launched from: a
/// double-click makes the two the same, a shortcut or a terminal elsewhere
/// does not. Falls back to the working directory when the path cannot be
/// resolved, which leaves the old behaviour rather than no behaviour.
pub fn exe_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// One shared client so repeated Gemini and LiveKit RoomService calls reuse
/// connections instead of renegotiating TLS per request. `reqwest::Client` is
/// already an `Arc` internally and is meant to be reused.
///
/// The timeout is a backstop, not a policy: without it a hung peer holds an
/// OAuth request handler open forever, and a stalled RoomService call parks the
/// agent's event loop mid-interview. Callers that need a tighter bound set
/// their own, as report generation does with `REPORT_TIMEOUT`.
pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            // Not `unwrap_or_default()`: that hands back a client with no
            // timeout at all, quietly dropping the backstop this exists for.
            .build()
            .expect("http client should build")
    })
}

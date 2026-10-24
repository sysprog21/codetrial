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

/// The same reading, in the signed type SQLite stores a timestamp in.
///
/// Every account and recording table spells a time as `INTEGER`, which rusqlite
/// binds from `i64`, so this conversion happened at eleven call sites as `as
/// i64`. An `as` cast between a wide unsigned and a signed type is the one that
/// wraps silently, and eleven of them are eleven places to check rather than
/// one. Saturating rather than wrapping, for the same reason the two clocks
/// above saturate: a clock far enough ahead to overflow this is a broken clock,
/// and a negative timestamp would read as 1969 and delete every row a sweep
/// looked at.
pub fn current_epoch_seconds_i64() -> i64 {
    i64::try_from(current_epoch_seconds()).unwrap_or(i64::MAX)
}

/// The receipt clock the evidence ledger is stamped with. One reading is taken
/// per packet at the boundary and passed down, so every entry a single event
/// produces shares it and the reducers themselves stay pure enough to replay.
/// Saturates rather than wrapping, because a clock behind the epoch is a broken
/// clock and not a reason to record a negative time.
pub fn current_epoch_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
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

/// Writes a new file that only its owner can read, and refuses rather than
/// replacing one: the config file Setup writes and the session key a
/// `--data-dir` launch makes both hold secrets.
///
/// `create_new`, so an existing name is reported rather than followed and
/// truncated. `is_cold_start` reached Setup by finding no config, and the
/// `is_file` it asked answers no for a symlink with nothing at the end of it:
/// without `O_EXCL` a dangling one planted there sends the credentials wherever
/// it points.
///
/// A file cut short is removed: the next launch would find it, skip Setup or
/// refuse the key, and never start again until someone deleted it by hand.
pub fn create_owner_only(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        // Otherwise the umask decides, and its usual answer is world-readable.
        // Windows has no equivalent here and inherits the folder's ACL.
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    std::io::Write::write_all(&mut file, contents).inspect_err(|_| {
        drop(file);
        let _ = std::fs::remove_file(path);
    })
}

/// Encode one URI component using only RFC 3986 unreserved bytes verbatim.
/// Both object paths and credential query values must escape slashes and
/// pluses.
pub(crate) fn percent_encode_component(value: &str) -> String {
    use std::fmt::Write as _;
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}

/// Hash ordered byte slices without allocating a concatenated input. Evidence
/// and integrity records share this encoding because their digests persist.
pub(crate) fn sha256_hex(parts: &[&[u8]]) -> String {
    use std::fmt::Write as _;
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    for part in parts {
        context.update(part);
    }
    let mut hex = String::with_capacity(64);
    for byte in context.finish().as_ref() {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
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
        http_client_builder()
            .build()
            .expect("http client should build")
    })
}

/// [`http_client`] that hands a redirect back instead of following it, for
/// requests that carry a credential. A 307/308 replays the request body at the
/// redirect target, which for the GitHub OAuth exchange includes the client
/// secret, and a followed redirect on an API call lets another host answer
/// for the one asked.
pub(crate) fn no_redirect_http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        http_client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("no-redirect http client should build")
    })
}

// Built and `expect`ed rather than `unwrap_or_default()`: that hands back a
// client with no timeout at all, quietly dropping the backstop this exists for.
pub(crate) fn http_client_builder() -> reqwest::ClientBuilder {
    bundled_roots_client_builder().timeout(std::time::Duration::from_secs(30))
}

/// The one TLS configuration for every connection this crate opens, HTTP and
/// the Gemini WebSocket alike: ring, and only the Mozilla roots compiled into
/// the binary, never the machine's certificate store.
pub(crate) fn tls_config() -> &'static std::sync::Arc<rustls::ClientConfig> {
    static CONFIG: std::sync::OnceLock<std::sync::Arc<rustls::ClientConfig>> =
        std::sync::OnceLock::new();
    CONFIG.get_or_init(|| {
        std::sync::Arc::new(
            rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("default TLS versions should be supported")
            .with_root_certificates(rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            })
            .with_no_client_auth(),
        )
    })
}

/// A client builder on [`tls_config`], with no timeout. Without it reqwest 0.13
/// fails to build a client on a host with no certificate store. Use
/// [`http_client`] unless a caller needs its own bounds.
pub fn bundled_roots_client_builder() -> reqwest::ClientBuilder {
    // reqwest takes an owned config; the clone shares the root store.
    reqwest::Client::builder().tls_backend_preconfigured(rustls::ClientConfig::clone(tls_config()))
}

/// Makes ring the process-wide rustls provider for LiveKit's transport, which
/// builds its TLS client without one. rustls otherwise picks a provider from
/// crate features, and panics once a dependency enables a second. Called at
/// startup, and again where a room is joined for hosts that skip `main`.
pub fn install_crypto_provider() {
    // An error only means a provider is already installed.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;

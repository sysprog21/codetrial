//! Serving `web/`: static files and their caching and content-type rules, the
//! generated runtime config the page reads its settings from, and the startup
//! warning for vendored assets nobody fetched.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

use super::AppState;
use super::policy::COMPILER_EXPLORER_ORIGIN;

/// Embeds browser assets, with disk overrides resolved per file.
/// Dotfiles are unservable and may contain partial downloads. VRM models are
/// fetched from a pinned upstream URL, so stale local copies must not bloat
/// releases.
#[derive(RustEmbed)]
#[folder = "web/"]
#[exclude = ".*"]
#[exclude = "**/.*"]
#[exclude = "**/*.vrm"]
struct EmbeddedWeb;

/// Warns about manifest-listed vendor files missing from the local tree.
/// A missing runtime degrades its feature rather than preventing server
/// startup.
/// Temporary web roots without checksum manifests remain silent.
pub(crate) fn warn_about_unfetched_vendor(web_dir: &Path) {
    let vendor = web_dir.join("vendor");
    let Ok(entries) = std::fs::read_dir(&vendor) else {
        return;
    };

    // The vendor root carries a manifest of its own, and the file it pins,
    // `livekit-client.js`, is the one whose absence leaves the interview page
    // unable to connect at all. Walking only the subdirectories would stay
    // silent about exactly the worst case.
    let directories = std::iter::once(vendor).chain(entries.flatten().map(|entry| entry.path()));
    let mut missing = Vec::new();
    for directory in directories {
        let Ok(manifest) = std::fs::read_to_string(directory.join("SHA256SUMS")) else {
            continue;
        };
        missing.extend(
            manifest
                .lines()
                // Trimmed because a CRLF checkout would otherwise hand every
                // name a trailing carriage return and report the whole manifest
                // as missing.
                .filter_map(|line| line.split_once("  ").map(|(_, name)| name.trim()))
                .filter(|name| !name.is_empty())
                .map(|name| directory.join(name))
                .filter(|path| !path.is_file())
                .map(|path| path.display().to_string()),
        );
    }
    if !missing.is_empty() {
        eprintln!(
            "{} vendored file(s) were never fetched, so the features that need them will fail in \
             the browser; run scripts/fetch-vendor.sh:",
            missing.len()
        );
        for path in missing {
            eprintln!("  {path}");
        }
    }
}

/// Resolves the first disk candidate with its metadata for resolver fixtures.
/// Shares URL normalization and refusal rules with the serving path.
pub async fn static_file_meta(
    root: &Path,
    path: &str,
) -> Option<(String, PathBuf, std::fs::Metadata)> {
    // Probe in order: most requests match the first candidate.
    for candidate in static_candidates(path)? {
        let path = root.join(&candidate);
        if let Some(metadata) = file_metadata(&path).await {
            return Some((candidate, path, metadata));
        }
    }
    None
}

/// Refuses dotfiles and syntax that can escape or change names on Windows.
/// ASCII filenames exclude drive prefixes, backslashes, and trailing spaces;
/// trailing dots are refused because Win32 strips them. Empty segments are
/// harmless.
fn is_refused_segment(part: &str) -> bool {
    part.starts_with('.')
        || part.ends_with('.')
        || !part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Ordered relative keys shared by disk and embedded stores.
/// Refused segments are rejected rather than sanitized into a different URL.
fn static_candidates(path: &str) -> Option<Vec<String>> {
    let clean = path.trim_start_matches('/');
    if clean.split('/').any(is_refused_segment) {
        return None;
    }

    // `PathBuf::join` collapses repeated separators for the disk tree. Do it
    // before looking in the embedded key set as well, or `/vendor//avatar/...`
    // serves only when a checkout happens to be present.
    let clean = clean
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/");

    // Matched on the path as sent: the page reads its set and task from
    // `location.pathname`, so a doubled slash it would misread is no task.
    if super::tasks::task_page(path).is_some() {
        return Some(vec!["task.html".to_owned()]);
    }

    if clean.is_empty() {
        return Some(vec!["index.html".to_string()]);
    }

    // An extension means the client asked for a specific file, so a miss is a
    // 404 rather than the single-page fallback below. Answering a missing `.js`
    // with `index.html` hands the browser HTML where it expects a script, and
    // the error it reports is a syntax error in a file that parsed fine.
    if Path::new(&clean).extension().is_some() {
        return Some(vec![clean]);
    }

    Some(vec![
        clean.clone(),
        format!("{clean}/index.html"),
        format!("{clean}.html"),
        "index.html".to_string(),
    ])
}

/// Keeps filesystem metadata probes off the async runtime's worker threads.
pub(crate) async fn file_metadata(path: &Path) -> Option<std::fs::Metadata> {
    tokio::fs::metadata(path)
        .await
        .ok()
        .filter(std::fs::Metadata::is_file)
}

pub(crate) async fn web_static_handler(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: header::HeaderMap,
) -> Response {
    let head_only = method == Method::HEAD;
    // Preconditions only short-circuit GET and HEAD (RFC 9110, 13.1.2).
    let headers = if matches!(method, Method::GET | Method::HEAD) {
        headers
    } else {
        header::HeaderMap::new()
    };

    // Where task mode is on, the home address is the default task: the learner
    // asked for task mode and has no assignment link to open, so what they get
    // is the task the program ships. The lobby is still at `/index.html`.
    if state.config.tasks.is_some()
        && matches!(method, Method::GET | Method::HEAD)
        && uri.path().trim_matches('/').is_empty()
    {
        return axum::response::Redirect::temporary(
            &crate::tasks::access::Assignment::built_in_path(),
        )
        .into_response();
    }

    // Candidate order outranks store order: an exact embedded match must beat
    // the disk index fallback. Disk-root existence is cached until restart.
    let Some(candidates) = static_candidates(uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    for candidate in candidates {
        if state.web_dir_exists
            && let Some(response) =
                disk_candidate(&state.config.web_dir, &candidate, &headers, head_only).await
        {
            return response;
        }
        if let Some(response) = embedded_candidate(&candidate, &headers, head_only) {
            return response;
        }
    }
    StatusCode::NOT_FOUND.into_response()
}

pub(crate) async fn runtime_config_handler(State(state): State<AppState>) -> Response {
    // The origin is emitted even when runs are enabled, so the page fetches
    // exactly what the Content-Security-Policy permits. A literal in the
    // browser could drift from the policy, and the failure would be a blocked
    // request rather than anything that names the cause.
    let mut body = if state.config.compiler_explorer_enabled {
        format!(
            "globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = true;\nglobalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = \"{COMPILER_EXPLORER_ORIGIN}\";\n"
        )
    } else {
        "globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = false;\nglobalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = \"\";\n".to_string()
    };

    // Whether to show the consent step at all, and which wording it agreed to.
    // The version travels this way rather than being written into the page,
    // because the server is the side that validates it and two spellings of a
    // version are one deploy away from disagreeing.
    body.push_str(&format!(
        "globalThis.CODETRIAL_RECORDING_ENABLED = {};\nglobalThis.CODETRIAL_CONSENT_VERSION = \"{}\";\nglobalThis.CODETRIAL_REPLAY_VERSION = {};\n",
        state.config.recording.is_some(),
        crate::recording::CONSENT_VERSION,
        crate::recording::REPLAY_VERSION
    ));

    // Whether the lobby offers Change keys: only the launch that may rewrite
    // its config, which the page cannot work out for itself.
    body.push_str(&format!(
        "globalThis.CODETRIAL_SETUP_ENABLED = {};\n",
        state.setup.is_some()
    ));
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

/// Revalidate ordinary assets without retransmitting unchanged bodies.
pub(crate) const STATIC_CACHE_CONTROL: HeaderValue = HeaderValue::from_static("no-cache");

/// Vendor upgrades keep filenames, so cache for a day rather than indefinitely.
pub(crate) const VENDOR_CACHE_CONTROL: HeaderValue =
    HeaderValue::from_static("public, max-age=86400");

/// A cheap weak validator from nanosecond modification time and length.
/// It avoids hashing large assets per request but can miss edits preserving
/// both.
pub(crate) fn file_etag(metadata: &std::fs::Metadata) -> Option<HeaderValue> {
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    HeaderValue::from_str(&format!("W/\"{}-{}\"", modified.as_nanos(), metadata.len())).ok()
}

/// Shared cache and content headers keep disk and embedded responses
/// consistent.
struct AssetHeaders {
    cache_control: HeaderValue,
    content_type: Option<HeaderValue>,
    etag: Option<HeaderValue>,
    length: u64,
}

impl AssetHeaders {
    /// Relative keys keep the vendor rule independent of the configured disk
    /// path.
    fn new(key: &str, etag: Option<HeaderValue>, length: u64) -> Self {
        Self {
            cache_control: if key.starts_with("vendor/") {
                VENDOR_CACHE_CONTROL
            } else {
                STATIC_CACHE_CONTROL
            },
            content_type: content_type(Path::new(key)),
            etag,
            length,
        }
    }

    /// Evaluates conditional GET/HEAD using weak comparison (RFC 9110, 13.1.2).
    /// Repeat cache policy, validator, and Vary so revalidation preserves them.
    fn not_modified(&self, request_headers: &header::HeaderMap) -> Option<Response> {
        // Bytes, not str: entity tags may carry obs-text, which `to_str`
        // rejects.
        let expected = self
            .etag
            .as_ref()
            .map(|tag| tag.as_bytes().strip_prefix(b"W/").unwrap_or(tag.as_bytes()));
        let matches = request_headers
            .get_all(header::IF_NONE_MATCH)
            .iter()
            .any(|value| {
                let value = value.as_bytes().trim_ascii();
                if value == b"*" {
                    return true;
                }
                let Some(expected) = expected else {
                    return false;
                };

                // Commas inside the opaque quoted value are not list
                // separators.
                let mut quoted = false;
                value
                    .split(|&byte| {
                        if byte == b'"' {
                            quoted = !quoted;
                        }
                        byte == b',' && !quoted
                    })
                    .any(|tag| {
                        let tag = tag.trim_ascii();
                        tag.strip_prefix(b"W/").unwrap_or(tag) == expected
                    })
            });
        matches.then(|| {
            let mut response = (
                StatusCode::NOT_MODIFIED,
                [
                    (header::CACHE_CONTROL, self.cache_control.clone()),
                    (header::VARY, HeaderValue::from_static("accept-encoding")),
                ],
            )
                .into_response();
            if let Some(etag) = &self.etag {
                response.headers_mut().insert(header::ETAG, etag.clone());
            }
            response
        })
    }

    /// HEAD uses the recorded length without reading a body.
    fn respond(self, body: Option<Body>) -> Response {
        let mut response = match body {
            Some(body) => body.into_response(),
            None => StatusCode::OK.into_response(),
        };
        let headers = response.headers_mut();
        if let Some(content_type) = self.content_type {
            headers.insert(header::CONTENT_TYPE, content_type);
        }
        headers.insert(header::CACHE_CONTROL, self.cache_control);
        if let Ok(length) = HeaderValue::from_str(&self.length.to_string()) {
            headers.insert(header::CONTENT_LENGTH, length);
        }
        if let Some(etag) = self.etag {
            headers.insert(header::ETAG, etag);
        }
        response
    }
}
/// Tries one resolved key against disk before the caller falls back to
/// embedding.
async fn disk_candidate(
    root: &Path,
    candidate: &str,
    request_headers: &header::HeaderMap,
    head_only: bool,
) -> Option<Response> {
    let path = root.join(candidate);
    let metadata = file_metadata(&path).await?;
    let headers = AssetHeaders::new(candidate, file_etag(&metadata), metadata.len());
    if let Some(not_modified) = headers.not_modified(request_headers) {
        return Some(not_modified);
    }
    if head_only {
        return Some(headers.respond(None));
    }
    // The file can still go away between the stat above and this read.
    let bytes = tokio::fs::read(&path).await.ok()?;
    Some(headers.respond(Some(Body::from(bytes))))
}

/// One already-resolved candidate against the embedded tree.
fn embedded_candidate(
    candidate: &str,
    request_headers: &header::HeaderMap,
    head_only: bool,
) -> Option<Response> {
    let file = EmbeddedWeb::get(candidate)?;

    // Strong, unlike the disk path's modification-time heuristic: these bytes
    // cannot change without a rebuild, and `rust-embed` already carries their
    // SHA-256, so nothing is hashed per request.
    let headers = AssetHeaders::new(
        candidate,
        Some(embedded_etag(&file)),
        file.data.len() as u64,
    );
    if let Some(not_modified) = headers.not_modified(request_headers) {
        return Some(not_modified);
    }
    if head_only {
        return Some(headers.respond(None));
    }
    Some(headers.respond(Some(Body::from(file.data))))
}

/// Uses half the embedded SHA-256 as a cache validator, not an integrity proof.
fn embedded_etag(file: &rust_embed::EmbeddedFile) -> HeaderValue {
    let digest = u128::from_be_bytes(
        file.metadata.sha256_hash()[..16]
            .try_into()
            .expect("16 bytes"),
    );
    HeaderValue::from_str(&format!("\"{digest:032x}\"")).expect("hex is a valid header value")
}

pub(crate) fn content_type(path: &Path) -> Option<HeaderValue> {
    let value = match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",

        // `.mjs` is pdf.js's own name for its API and worker. A module script
        // served as anything but JavaScript is refused, not sniffed.
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("wav") => "audio/wav",

        // `WebAssembly.instantiateStreaming` refuses anything that is not
        // `application/wasm`, and a body with no type at all is a coin toss for
        // any proxy in between.
        Some("wasm") => "application/wasm",

        // Same argument as `wasm`: a 15 MB untyped binary is exactly what a
        // proxy in between will try to sniff. GLTFLoader reads it as an
        // arraybuffer and does not care, but nothing else in the path is asked.
        Some("vrm" | "glb") => "model/gltf-binary",
        Some("tflite" | "binarypb") => "application/octet-stream",

        // Pyodide's stdlib. Same argument again: 2.3 MB with no type is the
        // kind of body a proxy in between decides to sniff, and `nosniff` on an
        // untyped response is a promise about nothing.
        Some("zip") => "application/zip",
        _ => return None,
    };
    Some(HeaderValue::from_static(value))
}

#[cfg(test)]
#[path = "../../tests/unit/web/assets.rs"]
mod tests;

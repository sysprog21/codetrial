//! Serving `web/`: static files and their caching and content-type rules, the
//! generated runtime config the page reads its settings from, and the startup
//! warning for vendored assets nobody fetched.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::AppState;
use super::policy::COMPILER_EXPLORER_ORIGIN;

/// Says out loud at startup which vendored assets were never downloaded.
///
/// The megabytes under `web/vendor` are pinned but not committed:
/// `scripts/fetch-vendor.sh` downloads them, and `make build`, `make serve` and
/// `make web` all run it first. `cargo run -- serve` does not, and neither does
/// a deployment that copies the tree without running the fetch. The failure
/// then lands on a candidate as a Python runtime that will not start or a face
/// detector that never loads, which is a long way from the cause.
///
/// A warning rather than a refusal: an interview without the browser Python
/// runner is degraded, not broken, and a server that will not start is worse
/// than one that says what is missing.
///
/// Silent when a vendor directory has no `SHA256SUMS`, which is what makes this
/// quiet for the temporary web roots the tests stand up. The actionable state
/// is a manifest present with its files absent.
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

pub async fn static_file(root: &Path, path: &str) -> Option<PathBuf> {
    static_file_meta(root, path).await.map(|(path, _)| path)
}

/// Whether a single path segment is one this server refuses to resolve.
///
/// An allowlist. Enumerating the spellings that escape is open ended, because
/// the escapes are whatever URL syntax permits, and the two that matter here
/// are neither `..` nor dot-prefixed: a backslash is a directory separator to
/// Windows and an ordinary path byte to the URI parser, and a colon makes a
/// segment drive-qualified, which `PathBuf::push` honors by discarding the
/// root it was joined onto. `/x\..\..\Cargo.toml` and `/C:/Windows/win.ini`
/// both walk out of the web root there.
///
/// Enumerating what the asset tree contains is not open ended. Every one of
/// the 367 names under `web/` is alphanumerics with a dot, underscore, or
/// dash, so the rule is that a segment must be an ordinary filename on every
/// platform this can be built for, and the allowlist costs no asset. It also
/// refuses the trailing dot and trailing space that Win32 strips, which would
/// otherwise name one file to the resolver and a different one to the disk.
///
/// Latent on the deployments that exist today, which are all Unix, where a
/// backslash is an ordinary filename character. It stops being latent the
/// moment a Windows build ships.
///
/// A leading dot covers `..` on its own, so there is no separate clause for
/// it. An empty segment is permitted because it cannot name anything.
fn is_refused_segment(part: &str) -> bool {
    part.starts_with('.')
        || part.ends_with('.')
        || !part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// The resolved path together with the `stat` that resolved it. Handed back
/// rather than thrown away because `static_response` needs the same metadata
/// for
/// the ETag and the content length, and asking the kernel twice for an answer
/// this walk already has is one syscall per request for nothing.
pub(crate) async fn static_file_meta(
    root: &Path,
    path: &str,
) -> Option<(PathBuf, std::fs::Metadata)> {
    let clean = path.trim_start_matches('/');
    if clean.split('/').any(is_refused_segment) {
        return None;
    }
    if !clean.is_empty() && Path::new(clean).extension().is_some() {
        let path = root.join(clean);
        return Some((path.clone(), file_metadata(&path).await?));
    }
    let candidates = if clean.is_empty() {
        vec![root.join("index.html")]
    } else {
        vec![
            root.join(clean),
            root.join(clean).join("index.html"),
            root.join(format!("{clean}.html")),
            root.join("index.html"),
        ]
    };

    // Sequential rather than joined: the first candidate answers almost every
    // request, and probing four paths in parallel to save a hit that rarely
    // happens costs three wasted stats on the one that usually does.
    for path in candidates {
        if let Some(metadata) = file_metadata(&path).await {
            return Some((path, metadata));
        }
    }
    None
}

/// `Path::is_file` is a blocking `stat`, and a route with no extension probes
/// up to four candidates. Four blocking syscalls on a runtime worker is four
/// too many when the async form is already used ten lines further down.
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
    static_response(
        &state.config.web_dir,
        uri.path(),
        headers.get(header::IF_NONE_MATCH),
        method == Method::HEAD,
    )
    .await
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

/// `no-store` on the whole asset tree was a correct instinct about
/// `/api/session` applied one level too wide: it forbids caching 12 MB of
/// pinned wasm, so a reload mid-interview re-downloads all of it. `no-cache`
/// still revalidates on every request, and the validator below turns that
/// revalidation into a 304 rather than a resend.
pub(crate) const STATIC_CACHE_CONTROL: HeaderValue = HeaderValue::from_static("no-cache");

/// Vendored MediaPipe is 11.8 MB the candidate pays for before the timer is
/// useful to them, and a day of it skips the revalidation round trips. Not
/// `immutable` for a year: `web/vendor/face-detection/README.md` tells
/// maintainers to overwrite these files in place, under the same names, so a
/// year-long promise is one the upgrade instructions cannot keep.
pub(crate) const VENDOR_CACHE_CONTROL: HeaderValue =
    HeaderValue::from_static("public, max-age=86400");

/// Modification time and length together change on any edit that matters, and
/// neither costs the file read or a digest of 5.5 MB of wasm per request. Weak
/// because it is a heuristic about the file rather than a hash of its bytes.
pub(crate) fn file_etag(metadata: &std::fs::Metadata) -> Option<HeaderValue> {
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    HeaderValue::from_str(&format!("W/\"{}-{}\"", modified.as_nanos(), metadata.len())).ok()
}

pub(crate) async fn static_response(
    root: &Path,
    path: &str,
    if_none_match: Option<&HeaderValue>,
    head_only: bool,
) -> Response {
    let Some((path, metadata)) = static_file_meta(root, path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // Anchored at the web root rather than matched anywhere in the string, so
    // an operator-supplied `CODETRIAL_WEB_DIR` that happens to contain the word
    // cannot cache the whole tree. Decided before the 304 branch because a 304
    // replaces the stored headers: answering a revalidation with `no-cache`
    // would downgrade the copy the browser already holds and undo the day.
    let cache_control = if path
        .strip_prefix(root)
        .is_ok_and(|relative| relative.starts_with("vendor"))
    {
        VENDOR_CACHE_CONTROL
    } else {
        STATIC_CACHE_CONTROL
    };
    let etag = file_etag(&metadata);

    if let Some(etag) = &etag
        && Some(etag) == if_none_match
    {
        // A 304 has to repeat the validator and the `Vary` the 200 would have
        // sent. Without the validator a cache cannot tell which version it just
        // confirmed, and without `Vary` a shared cache forgets that the stored
        // body is one specific encoding and can hand gzip to a client that
        // never asked for it.
        //
        // Ahead of the HEAD branch, not after it: a conditional HEAD is a
        // revalidation like any other, and answering it 200 tells the browser
        // its cached copy was replaced when nothing changed.
        return (
            StatusCode::NOT_MODIFIED,
            [
                (header::CACHE_CONTROL, cache_control),
                (header::ETAG, etag.clone()),
                (header::VARY, HeaderValue::from_static("accept-encoding")),
            ],
        )
            .into_response();
    }

    // Answered from metadata, never from the body. Axum routes HEAD to the same
    // handler, so the avatar's "is a model published" probe would otherwise
    // read a 15 MB file into memory once per session and throw it away.
    if head_only {
        let mut response = StatusCode::OK.into_response();
        let response_headers = response.headers_mut();
        if let Some(content_type) = content_type(&path) {
            response_headers.insert(header::CONTENT_TYPE, content_type);
        }
        response_headers.insert(header::CACHE_CONTROL, cache_control);
        if let Ok(length) = HeaderValue::from_str(&metadata.len().to_string()) {
            response_headers.insert(header::CONTENT_LENGTH, length);
        }
        if let Some(etag) = etag {
            response_headers.insert(header::ETAG, etag);
        }
        return response;
    }

    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mut response = Body::from(bytes).into_response();
            let headers = response.headers_mut();
            if let Some(content_type) = content_type(&path) {
                headers.insert(header::CONTENT_TYPE, content_type);
            }
            headers.insert(header::CACHE_CONTROL, cache_control);
            if let Some(etag) = etag {
                headers.insert(header::ETAG, etag);
            }
            response
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

pub(crate) fn content_type(path: &Path) -> Option<HeaderValue> {
    let value = match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",

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
mod tests {
    use super::is_refused_segment;

    /// Asserts the refusal itself rather than a served request, because a
    /// served request cannot distinguish the two on this platform: a backslash
    /// and a colon are ordinary filename bytes to Unix, so every one of these
    /// answers 404 for want of a file whether or not the resolver refused it.
    /// The refusal is what a Windows build would depend on.
    #[test]
    fn a_segment_must_be_an_ordinary_filename() {
        for refused in [
            "..",
            ".git",
            ".env.local",
            "x\\..\\..\\Cargo.toml",
            "\\..\\Cargo.toml",
            "C:",
            "index.html:$DATA",
            "index.html.",
            "with space",
        ] {
            assert!(is_refused_segment(refused), "{refused} should be refused");
        }

        for allowed in [
            "",
            "index.html",
            "three-vrm.js",
            "face_detection.js",
            "jim.vrm",
        ] {
            assert!(!is_refused_segment(allowed), "{allowed} should resolve");
        }
    }
}

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

/// The release binary carries the complete browser application. Disk assets
/// still take precedence per file, so editing `web/` remains the local
/// development workflow, and a tree that is merely incomplete is filled in
/// rather than being all-or-nothing: running the binary beside a checkout that
/// never fetched `web/vendor` serves the vendored megabytes from inside the
/// executable instead of answering 404 with a good copy in hand.
///
/// Dotfiles are excluded because the resolver below refuses to serve any path
/// segment starting with `.`, so embedding them is weight nobody can fetch. A
/// killed `scripts/fetch-vendor.sh` leaves multi-megabyte `.part` files, and
/// without this they land in `.rodata`.
///
/// Models are excluded for the same reason and one more. Nothing fetches one
/// into this tree any more, and no page requests one: the browser gets the
/// avatar from the pinned upstream URL in `web/avatar/model.js`. The rule stays
/// anyway, because it is what makes the binary's size independent of whoever
/// still has a 10.9 MB copy left over from before the change. Keyed by
/// extension rather than by that one filename, so it states the property
/// `no_model_is_embedded_in_the_binary` actually asserts.
#[derive(RustEmbed)]
#[folder = "web/"]
#[exclude = ".*"]
#[exclude = "**/.*"]
#[exclude = "**/*.vrm"]
struct EmbeddedWeb;

/// Says out loud at startup which vendored assets were never downloaded.
///
/// The megabytes under `web/vendor` are pinned but not committed:
/// `scripts/fetch-vendor.sh` downloads them, and `make build` and `make web`
/// run it first. `cargo run -- web` does not, and neither does a deployment
/// that copies the tree without running the fetch. The failure
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

/// The first candidate that exists on disk, with the `stat` that found it.
///
/// The handler resolves one candidate at a time now, because candidate order
/// has to beat store order, so nothing on the request path walks the whole list
/// against a single store any more. What is left is the shape the resolution
/// tests want: a question about which file a URL names, answered without a
/// server. It shares `static_candidates` with the handler, so the rule that
/// decides what a URL may name still has one owner and these tests still ask
/// the real one.
pub async fn static_file_meta(
    root: &Path,
    path: &str,
) -> Option<(String, PathBuf, std::fs::Metadata)> {
    // Sequential rather than joined: the first candidate answers almost every
    // request, and probing four paths in parallel to save a hit that rarely
    // happens costs three wasted stats on the one that usually does.
    //
    // The candidate is handed back beside the path it resolved to. It is what
    // decides the caching policy, and reconstructing it afterwards with
    // `strip_prefix(root)` was the second spelling of a rule this file already
    // argues must have only one.
    for candidate in static_candidates(path)? {
        let path = root.join(&candidate);
        if let Some(metadata) = file_metadata(&path).await {
            return Some((candidate, path, metadata));
        }
    }
    None
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
/// Load-bearing rather than theoretical since the release job below started
/// publishing a Windows binary. On Unix a backslash is an ordinary filename
/// character and these answer 404 for want of a file; on Windows they would
/// resolve.
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

/// The relative paths a URL may resolve to, in the order to try them, or `None`
/// for a URL the server refuses to resolve at all.
///
/// One list for both stores. They spelled these rules out separately until the
/// trailing slash proved why that does not hold: the disk side normalizes
/// `foo//index.html` through `PathBuf::join` and the embedded side matched no
/// key, so the same URL answered differently depending on which store happened
/// to hold the file. A rule written twice is a rule that drifts, and the drift
/// is invisible until someone requests the one URL that separates them.
///
/// Refusing rather than sanitizing: `..` and a leading `.` on any segment have
/// no reading this server wants to serve, and a resolver that quietly rewrites
/// a path into a legal one is a resolver nobody can predict from the URL.
///
/// A backslash is refused with them, because on Windows it is a directory
/// separator and here it is not: `http` accepts `\` as an ordinary path byte,
/// so `/x\..\..\Cargo.toml` arrives as one segment that is neither `..` nor
/// dot-prefixed, and `Path::join` then walks it straight out of the web root.
/// The released Windows executable is what makes that reachable. No vendored
/// filename contains one, so refusing costs nothing.
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

    // `jim.vrm` used to be fetched into checkouts and disk assets override the
    // embedded store. It is now browser-cached from its pinned source, so
    // refuse the retired URL even when an old ignored file remains on disk.
    //
    // Case-insensitively, because the comparison has to be at least as
    // forgiving as the filesystem underneath it. macOS and Windows both resolve
    // `JIM.VRM` to the leftover file, so an exact match refused one spelling
    // and served 10.9 MB for every other. ASCII is the whole alphabet a
    // vendored filename may use, which `is_refused_segment` already enforces.
    if clean.eq_ignore_ascii_case("vendor/avatar/jim.vrm") {
        return None;
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
    let if_none_match = headers.get(header::IF_NONE_MATCH);
    let head_only = method == Method::HEAD;

    // Per file rather than per tree. Asking `web_dir.is_dir()` here was the
    // all-or-nothing form, and it is true for anyone running the binary from a
    // directory that merely happens to contain a `web/`, so a half-populated
    // tree answered 404 for assets the executable was carrying.
    //
    // The question is still asked once, at startup, because the alternative is
    // a `tokio::fs::metadata` hop to the blocking pool on every request that a
    // released install is guaranteed to lose: it has no `web/`, so the disk
    // store never answers and the probe is pure waste. Once at startup keeps
    // the per-file fallback wherever the directory does exist. The cost is that
    // creating `web/` under a running server needs a restart to take, which is
    // the more predictable behavior anyway. Candidate order beats store order.
    // Exhausting the disk tree first let its single-page fallback,
    // `index.html`, answer for a candidate the embed could have matched
    // exactly: a directory holding one stale `index.html` served it for
    // `/interview` with a 200, because the disk's fourth candidate was tried
    // before the embed's third. That is the wrong page rather than a missing
    // one, which is the failure this resolver argues everywhere else that it
    // exists to avoid.
    //
    // Nothing changes for the two shapes that are not half-populated. A
    // checkout answers from disk on the same candidate it always did, and a
    // released binary has no `web/` at all, so the disk store is never asked.
    let Some(candidates) = static_candidates(uri.path()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    for candidate in candidates {
        if state.web_dir_exists
            && let Some(response) =
                disk_candidate(&state.config.web_dir, &candidate, if_none_match, head_only).await
        {
            return response;
        }
        if let Some(response) = embedded_candidate(&candidate, if_none_match, head_only) {
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

/// Everything about an asset response that does not depend on which store the
/// bytes came from: the caching policy, the validator, and the content type.
///
/// Built once from the relative key both stores resolve to. Before this the
/// disk and embedded paths each spelled out the whole contract, and they had
/// already drifted: the vendor rule was a component-wise `Path::starts_with`
/// on one side and a string prefix on the other, and only one of the two set
/// `Content-Length` on a body response. `static_candidates` exists because a
/// rule written twice drifts; this is the other half of that argument.
struct AssetHeaders {
    cache_control: HeaderValue,
    content_type: Option<HeaderValue>,
    etag: Option<HeaderValue>,
    length: u64,
}

impl AssetHeaders {
    /// `key` is relative to the tree root, so the vendor test is anchored
    /// there by construction. Matching anywhere in an absolute path would let
    /// an operator-supplied `CODETRIAL_WEB_DIR` that happens to contain the
    /// word cache the whole tree.
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

    /// A 304 when the client already holds this exact copy.
    ///
    /// Decided before the body is fetched, and before the HEAD branch: a
    /// conditional HEAD is a revalidation like any other, and answering it 200
    /// tells the browser its cached copy was replaced when nothing changed.
    ///
    /// A 304 replaces the stored headers, so it has to repeat both the
    /// validator and the `Vary` the 200 would have sent. Without the validator
    /// a cache cannot tell which version it just confirmed, and without `Vary`
    /// a shared cache forgets the stored body is one specific encoding and can
    /// hand gzip to a client that never asked for it. Repeating
    /// `cache_control` matters for the same reason: answering a vendor
    /// revalidation with `no-cache` would undo the day the browser was given.
    fn not_modified(&self, if_none_match: Option<&HeaderValue>) -> Option<Response> {
        let etag = self.etag.as_ref()?;
        (Some(etag) == if_none_match).then(|| {
            (
                StatusCode::NOT_MODIFIED,
                [
                    (header::CACHE_CONTROL, self.cache_control.clone()),
                    (header::ETAG, etag.clone()),
                    (header::VARY, HeaderValue::from_static("accept-encoding")),
                ],
            )
                .into_response()
        })
    }

    /// `None` is a HEAD, answered from the recorded length and never from the
    /// body. Axum routes HEAD to the same handler, so the avatar's "is a model
    /// published" probe would otherwise read a 15 MB file into memory once per
    /// session and throw it away.
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
/// One already-resolved candidate against the disk tree.
///
/// The candidate arrives resolved because `static_candidates` runs once for
/// both stores: expanding it again here would be a second copy of the rule that
/// decides what a URL may name, which is the one rule in this file that must
/// not have two.
async fn disk_candidate(
    root: &Path,
    candidate: &str,
    if_none_match: Option<&HeaderValue>,
    head_only: bool,
) -> Option<Response> {
    let path = root.join(candidate);
    let metadata = file_metadata(&path).await?;
    let headers = AssetHeaders::new(candidate, file_etag(&metadata), metadata.len());
    if let Some(not_modified) = headers.not_modified(if_none_match) {
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
    if_none_match: Option<&HeaderValue>,
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
    if let Some(not_modified) = headers.not_modified(if_none_match) {
        return Some(not_modified);
    }
    if head_only {
        return Some(headers.respond(None));
    }
    Some(headers.respond(Some(Body::from(file.data))))
}

/// Sixteen bytes of the digest rather than all thirty-two: this is a cache
/// validator, not a signature, and a collision costs one stale asset that a
/// rebuild fixes.
///
/// Formatted as one big-endian integer rather than byte by byte. A hand-rolled
/// loop over shifts and masks says the same thing in more places that can be
/// wrong, and it hands the mutation gate four operators to flip where this
/// leaves none.
///
/// Infallible, so callers do not branch on a None that cannot happen: hex
/// digits between quotes are always a valid header value.
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
    use super::{EmbeddedWeb, embedded_etag, is_refused_segment, static_candidates};

    /// Normalization is tested here rather than through a served request
    /// because a served request cannot see it. `EmbeddedWeb::get` matches keys
    /// exactly only in release; a debug or test build reads the same names off
    /// the filesystem, where the kernel collapses repeated separators before
    /// the lookup ever happens. So the one store whose behavior depends on this
    /// function is the one store no test binary runs.
    ///
    /// Testing the rule instead of the store sidesteps that: this is a pure
    /// function of the URL, and it is wrong or right identically in both.
    #[test]
    fn repeated_separators_collapse_to_one_key() {
        let direct = static_candidates("/vendor/avatar/three-vrm.js").unwrap();
        for spelling in [
            "/vendor//avatar/three-vrm.js",
            "//vendor/avatar/three-vrm.js",
            "/vendor///avatar//three-vrm.js",
            "/vendor/avatar/three-vrm.js/",
        ] {
            assert_eq!(
                static_candidates(spelling).unwrap(),
                direct,
                "{spelling} should resolve to the same key as the plain spelling"
            );
        }
    }

    /// The 10.9 MB model is the largest single thing that could land in the
    /// binary, and the `#[exclude]` on `EmbeddedWeb` is the only thing keeping
    /// it out. That cannot be shown over HTTP any more: `static_candidates`
    /// refuses the retired URL outright, so a 404 there is the refusal talking
    /// and says nothing about what was embedded. Ask the store directly.
    ///
    /// Silent on a machine that has no leftover copy, which is most of them.
    /// It earns its place on the one that does, and on the day someone drops a
    /// replacement model into `web/` without reading why the exclusion exists.
    ///
    /// What it checks in a debug build is the filter, not the bytes. Without
    /// `debug-embed`, `rust-embed` resolves from disk here, and its dynamic
    /// implementation applies the same `#[exclude]`, so an excluded path is
    /// absent either way. The filter is what can actually break: a glob that
    /// stops matching puts 10.9 MB back into release with nothing else failing.
    /// Confirmed by deleting the exclusion with the model present, which fails
    /// this. Gating it to release would leave the glob untested in the build
    /// everybody runs.
    #[test]
    fn no_model_is_embedded_in_the_binary() {
        assert!(
            EmbeddedWeb::get("vendor/avatar/jim.vrm").is_none(),
            "the release binary must not carry the avatar model"
        );
        let embedded: Vec<String> = EmbeddedWeb::iter()
            .filter(|path| path.ends_with(".vrm"))
            .map(|path| path.to_string())
            .collect();
        assert!(
            embedded.is_empty(),
            "the browser fetches the model from its pinned source, so none belongs here: {embedded:?}"
        );
    }

    #[test]
    fn retired_avatar_model_is_never_served_from_disk() {
        for path in [
            "/vendor/avatar/jim.vrm",
            "/vendor//avatar/jim.vrm",
            "//vendor/avatar/jim.vrm",
            // Each of these served the whole 10.9 MB on macOS and Windows
            // before the comparison was made case-insensitive: the filesystem
            // resolved what the string compare had just declined to match.
            "/vendor/avatar/JIM.VRM",
            "/vendor/avatar/Jim.Vrm",
            "/VENDOR/AVATAR/JIM.VRM",
        ] {
            assert!(
                static_candidates(path).is_none(),
                "{path} should be refused"
            );
        }
    }

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
            "model.vrm",
        ] {
            assert!(!is_refused_segment(allowed), "{allowed} should resolve");
        }
    }

    /// The empty-segment filter runs after the refusal, never before, so a
    /// separator cannot be used to smuggle a segment past it. Collapsing first
    /// would be the bug: nothing here may become legal by being rewritten.
    ///
    /// Sole owner of the backslash case, and it has to be. That byte is a
    /// directory separator to Windows and an ordinary filename character to
    /// every runner that could serve a request against it, so a served-request
    /// check 404s on Linux either way and proves nothing. Asserting the refusal
    /// directly is what makes the Windows artifact the diff added safe.
    #[test]
    fn normalization_cannot_launder_a_refused_path() {
        for path in [
            "/../Cargo.toml",
            "/a//../../Cargo.toml",
            "/..//..//Cargo.toml",
            "//../Cargo.toml",
            "/./Cargo.toml",
            "/.git/config",
            "/vendor//.hidden",
            // A backslash is an ordinary path byte to `http` and a directory
            // separator to Windows, so a segment carrying one is neither `..`
            // nor dot-prefixed here and still walks out of the web root there.
            // Unreachable until the release shipped a Windows executable, and
            // untestable from a Unix runner, which is why it is asserted on the
            // rule rather than on a served request.
            "/x\\..\\..\\Cargo.toml",
            "/\\..\\Cargo.toml",
            "/vendor\\..\\..\\Cargo.toml",
            // Drive-qualified, so `PathBuf::push` discards the web root it was
            // joined onto and reads from the volume instead. Neither `..` nor
            // dot-prefixed, and it never had a backslash in it.
            "/C:/Windows/win.ini",
            "/c:/Windows/win.ini",
            "/vendor/C:/Windows/win.ini",
            // An NTFS alternate data stream on a file that does exist.
            "/index.html:$DATA",
        ] {
            assert!(
                static_candidates(path).is_none(),
                "{path} should be refused outright, not resolved"
            );
        }
    }

    /// An extension means the client asked for one file, so a miss is a 404.
    /// The single-page fallback would answer a missing `.js` with HTML, and the
    /// browser reports that as a syntax error in a file that parsed fine.
    #[test]
    fn an_extension_suppresses_the_index_fallback() {
        assert_eq!(
            static_candidates("/missing.js").unwrap(),
            vec!["missing.js".to_string()]
        );
        assert!(
            static_candidates("/missing")
                .unwrap()
                .contains(&"index.html".to_string())
        );
    }

    /// The validator has to be the digest of the bytes actually served, not
    /// merely stable across two requests. A round trip through If-None-Match
    /// compares the server against itself and passes on any value it likes,
    /// so this compares it against the file instead.
    #[test]
    fn the_etag_is_the_leading_half_of_the_file_digest() {
        use sha2::{Digest, Sha256};

        let file = EmbeddedWeb::get("index.html").expect("the index is embedded");
        let mut expected = String::from('"');
        for byte in &Sha256::digest(&file.data)[..16] {
            expected.push_str(&format!("{byte:02x}"));
        }
        expected.push('"');
        assert_eq!(embedded_etag(&file), expected.as_str());
    }

    /// The root and a bare directory both end at the index, by different
    /// routes: one has no candidates to try, the other exhausts its own first.
    #[test]
    fn a_bare_route_falls_back_to_the_index() {
        assert_eq!(
            static_candidates("/").unwrap(),
            vec!["index.html".to_string()]
        );
        assert_eq!(
            static_candidates("/interview").unwrap(),
            vec![
                "interview".to_string(),
                "interview/index.html".to_string(),
                "interview.html".to_string(),
                "index.html".to_string(),
            ]
        );
    }
}

//! The `tests` module of `src/web/assets.rs`, which declares this file by path.
//! Everything here reaches into `src/web/assets.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

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

/// Avatar models come from their pinned upstream URL rather than the embedded
/// web tree. Assert the filter directly, because the browser fetch path cannot
/// prove which files a release binary carries.
#[test]
fn no_model_is_embedded_in_the_binary() {
    let embedded: Vec<String> = EmbeddedWeb::iter()
        .filter(|path| path.ends_with(".vrm"))
        .map(|path| path.to_string())
        .collect();
    assert!(
        embedded.is_empty(),
        "the browser fetches the model from its pinned source, so none belongs here: {embedded:?}"
    );
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
        // separator to Windows, so a segment carrying one is neither `..` nor
        // dot-prefixed here and still walks out of the web root there.
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

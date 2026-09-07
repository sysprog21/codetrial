//! The `tests` module of `src/delivery.rs`, which declares this file by path.
//! Everything here reaches into `src/delivery.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::*;

#[test]
fn dates_are_rfc_3339_in_utc() {
    // The shape Drive wants, and the arithmetic that produces it, checked
    // against dates a leap year and a century get wrong when it is written by
    // hand.
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339(1_771_632_000), "2026-02-21T00:00:00Z");
    assert_eq!(
        rfc3339(1_771_632_000 + 24 * 60 * 60),
        "2026-02-22T00:00:00Z"
    );
    // 2000 was a leap year and 1900 was not; the day after 2000-02-28.
    assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(rfc3339(1_767_225_599), "2025-12-31T23:59:59Z");
}

#[test]
fn object_names_survive_their_slashes() {
    // The JSON API takes the whole object name as one path segment, so a prefix
    // that arrived unescaped would name a different object, or none.
    assert_eq!(
        encode_path("codetrial/abc123.mp4"),
        "codetrial%2Fabc123.mp4"
    );
    assert_eq!(encode_path("a b"), "a%20b");
    assert_eq!(encode_path("plain-name_1.mp4"), "plain-name_1.mp4");
}

#[test]
fn a_deletion_that_finds_nothing_succeeded() {
    assert!(gone_or_ok(204, "x").is_ok());
    assert!(
        gone_or_ok(404, "x").is_ok(),
        "retention runs more than once"
    );
    assert!(gone_or_ok(403, "x").is_err());
}

/// `bytes=0-n`, where `n` is the last byte the session holds.
fn with_range(range: Option<&str>) -> reqwest::Response {
    let mut builder = axum::http::Response::builder().status(308);
    if let Some(range) = range {
        builder = builder.header(axum::http::header::RANGE, range);
    }
    reqwest::Response::from(builder.body(String::new()).unwrap())
}

/// The offset a `308` moves the upload to. Everything here is one
/// subtraction away from writing a file with a hole in it: the loop trusts
/// this number and never re-sends what it says arrived.
#[test]
fn a_resume_offset_is_believed_only_when_it_can_be_read_exactly() {
    // A session that has nothing yet sends no header at all, which is not an
    // error and must not be read as one.
    assert_eq!(stored_bytes(&with_range(None), 1_000).unwrap(), 0);

    // Inclusive end, so the count is one more than the index. Off by one here
    // re-sends a byte or skips one, and only the second is visible.
    assert_eq!(
        stored_bytes(&with_range(Some("bytes=0-0")), 1_000).unwrap(),
        1
    );
    assert_eq!(
        stored_bytes(&with_range(Some("bytes=0-999")), 1_000).unwrap(),
        1_000
    );
    assert_eq!(
        stored_bytes(&with_range(Some("bytes=0-42 ")), 1_000).unwrap(),
        43,
        "a trailing space is still a readable range"
    );

    // Anything that is not `bytes=0-n` is a header this code cannot act on.
    // Guessing at one moves the offset past bytes that were never stored, so
    // each of these has to be an error and not a zero.
    for range in [
        "bytes=500-999",
        "bytes=0-abc",
        "bytes=0-",
        "0-999",
        "",
        "bytes=0--1",
    ] {
        assert!(
            stored_bytes(&with_range(Some(range)), 1_000).is_err(),
            "{range:?} must not be guessed at"
        );
    }

    // `end + 1` on the largest value a range can name. Wrapping it would report
    // an empty session and restart a finished upload from zero.
    assert!(
        stored_bytes(&with_range(Some(&format!("bytes=0-{}", u64::MAX))), 1_000).is_err(),
        "an offset that overflows must be refused"
    );

    // More than the object has means the session is not the one this transfer
    // opened. Believing it finishes a truncated file.
    assert!(
        stored_bytes(&with_range(Some("bytes=0-1000")), 1_000).is_err(),
        "a session cannot hold more than the object has"
    );
}

#[test]
fn a_service_account_without_a_key_is_refused_at_startup() {
    let now = Arc::new(|| 0);
    assert!(
        GoogleDelivery::new("{}", "bucket", "drive", now.clone()).is_err(),
        "a credential this broken must stop a deployment, not an interview"
    );
    assert!(GoogleDelivery::new("not json", "bucket", "drive", now.clone()).is_err());

    // Base64 that decodes and is not a key. Without the `ring` check this was
    // accepted at startup and failed at the first delivery, which is the
    // failure the doc comment above `new` promises not to have.
    let pretend = serde_json::json!({
        "client_email": "codetrial@example.iam.gserviceaccount.com",
        "private_key": "-----BEGIN PRIVATE KEY-----\nbm90IGEga2V5\n-----END PRIVATE KEY-----\n",
    })
    .to_string();
    assert!(GoogleDelivery::new(&pretend, "bucket", "drive", now).is_err());
}

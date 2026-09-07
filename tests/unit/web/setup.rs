//! The `tests` module of `src/web/setup.rs`, which declares this file by path.
//! Everything here reaches into `src/web/setup.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::host_is_loopback;
use axum::http::HeaderValue;

fn host(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).expect("a test Host should be a header value")
}

#[test]
fn a_loopback_host_naming_the_bound_port_is_accepted() {
    for value in [
        "127.0.0.1:3000",
        "localhost:3000",
        "[::1]:3000",
        "LocalHost:3000",
    ] {
        assert!(host_is_loopback(Some(&host(value)), 3000), "{value}");
    }
}

/// The rebinding case: the name is the attacker's, the port is ours, and
/// the port is the half that matches.
#[test]
fn a_host_that_is_not_a_loopback_name_is_refused() {
    for value in [
        "codetrial.example:3000",
        "127.0.0.1.codetrial.example:3000",
        "localhost.codetrial.example:3000",
    ] {
        assert!(!host_is_loopback(Some(&host(value)), 3000), "{value}");
    }
}

/// A `Host` is only missing or portless from something that is not the
/// browser this page was opened in, since the console printed the port.
#[test]
fn a_missing_or_mismatched_port_is_refused() {
    assert!(!host_is_loopback(None, 3000));
    assert!(!host_is_loopback(Some(&host("127.0.0.1")), 3000));
    assert!(!host_is_loopback(Some(&host("127.0.0.1:3001")), 3000));
    assert!(!host_is_loopback(Some(&host("[::1]")), 3000));
    assert!(!host_is_loopback(Some(&host("127.0.0.1:")), 3000));
}

/// Port 80 is the one a browser leaves out of the `Host`, so it is the one
/// case where a portless value is the real thing rather than a hand-written
/// request.
#[test]
fn the_default_http_port_is_accepted_without_one() {
    assert!(host_is_loopback(Some(&host("localhost")), 80));
    assert!(host_is_loopback(Some(&host("localhost:80")), 80));
    assert!(!host_is_loopback(Some(&host("localhost")), 3000));

    // The bracket guard's case: the last colon of `[::1]` is inside the
    // brackets, and reading it as the separator refuses a real browser.
    assert!(host_is_loopback(Some(&host("[::1]")), 80));
}

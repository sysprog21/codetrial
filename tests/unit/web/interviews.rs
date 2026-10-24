use super::report_owner_matches;
use axum::http::{HeaderMap, HeaderValue};

fn with_owner(value: &'static str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-codetrial-report-owner", HeaderValue::from_static(value));
    headers
}

#[test]
fn same_account_allows_idempotent_retry() {
    assert!(report_owner_matches(&with_owner("7"), 7));
}

#[test]
fn changed_account_is_rejected() {
    assert!(!report_owner_matches(&with_owner("7"), 8));
}

#[test]
fn malformed_owner_is_rejected() {
    for value in ["wrong", "0", "-1", "7.0", ""] {
        assert!(!report_owner_matches(&with_owner(value), 7));
    }
}

#[test]
fn legacy_lobby_delete_needs_no_owner_header() {
    assert!(report_owner_matches(&HeaderMap::new(), 7));
}

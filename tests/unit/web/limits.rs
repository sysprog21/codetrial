//! The `tests` module of `src/web/limits.rs`, which declares this file by path.

use super::*;
use crate::web::{MAX_BODY_BYTES, TOKEN_RATE_LIMIT};

fn ip(last: u8) -> IpAddr {
    IpAddr::from([127, 0, 0, last])
}

#[tokio::test]
async fn token_body_read_has_a_deadline() {
    let stalled = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
    let body = Body::from_stream(stalled);

    assert_eq!(
        read_token_body(body, Duration::from_millis(10), MAX_BODY_BYTES).await,
        Err(TokenBodyReadError::TimedOut),
    );
}

/// The routes reachable without a credential read their body through the same
/// deadline, and answer a stalled client with a status instead of holding the
/// connection.
#[tokio::test(start_paused = true)]
async fn a_stalled_body_is_refused_with_a_timeout_status() {
    let stalled = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
    let refusal = read_body_or_refuse(Body::from_stream(stalled), MAX_BODY_BYTES)
        .await
        .expect_err("a body that never finishes is refused");
    assert_eq!(refusal.status(), StatusCode::REQUEST_TIMEOUT);

    let oversized = Body::from(vec![0_u8; MAX_BODY_BYTES + 1]);
    let refusal = read_body_or_refuse(oversized, MAX_BODY_BYTES)
        .await
        .expect_err("a body past the cap is refused");
    assert_eq!(refusal.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let body = read_body_or_refuse(Body::from("{}"), MAX_BODY_BYTES)
        .await
        .expect("a small body is read");
    assert_eq!(&body[..], b"{}");
}

#[test]
fn rate_limit_window_expires_so_a_blocked_client_recovers() {
    let limit = TokenRateLimit::with_limit(TOKEN_RATE_LIMIT);
    let start = Instant::now();

    for attempt in 1..=TOKEN_RATE_LIMIT {
        assert!(limit.allow(ip(1), start), "attempt {attempt} should pass");
    }
    assert!(!limit.allow(ip(1), start), "the budget is spent");

    // Still inside the window: hammering must not extend the block past it.
    let late = start + TOKEN_RATE_WINDOW - Duration::from_secs(1);
    assert!(!limit.allow(ip(1), late));

    // A blocked client has to recover, or one burst bans them forever.
    assert!(limit.allow(ip(1), start + TOKEN_RATE_WINDOW));
}

/// The sweep runs at most once per window, so an entry can outlive its own
/// window while no sweep is due. Without the per-entry reset that client
/// stays blocked until the next sweep happens to come round.
#[test]
fn a_window_expires_even_when_no_sweep_is_due() {
    let limit = TokenRateLimit::with_limit(TOKEN_RATE_LIMIT);
    let start = Instant::now();

    // Spend one client's budget early, then force a sweep it survives, so the
    // next sweep is a full window away.
    for _ in 0..TOKEN_RATE_LIMIT {
        assert!(limit.allow(ip(1), start + Duration::from_secs(50)));
    }
    assert!(!limit.allow(ip(1), start + Duration::from_secs(50)));
    assert!(limit.allow(ip(2), start + TOKEN_RATE_WINDOW));
    assert_eq!(
        limit.state.lock().unwrap().windows.len(),
        2,
        "the blocked client is still inside its window and must survive the sweep"
    );

    // 65s after it started: its window is over, but only 55s since the sweep,
    // so no sweep is due.
    assert!(
        limit.allow(ip(1), start + Duration::from_secs(115)),
        "an expired window must reset itself rather than wait for a sweep"
    );
}

#[test]
fn rate_limit_is_per_client_and_prunes_expired_windows() {
    let limit = TokenRateLimit::with_limit(TOKEN_RATE_LIMIT);
    let start = Instant::now();

    for _ in 0..TOKEN_RATE_LIMIT {
        assert!(limit.allow(ip(1), start));
    }
    assert!(!limit.allow(ip(1), start), "first client is spent");
    assert!(
        limit.allow(ip(2), start),
        "a second client must not inherit the first client's budget"
    );

    // The map is only bounded by pruning, so an idle client must not linger.
    assert_eq!(limit.state.lock().unwrap().windows.len(), 2);
    assert!(limit.allow(ip(3), start + TOKEN_RATE_WINDOW));
    assert_eq!(
        limit.state.lock().unwrap().windows.len(),
        1,
        "expired windows should be dropped, not accumulated"
    );
}

/// A route that answers an oversized body in its own shape still gives a
/// stalled one the same timeout, so no route holds a connection open for a
/// client that has stopped sending.
#[tokio::test(start_paused = true)]
async fn a_route_keeps_its_own_oversize_answer_and_gets_the_timeout() {
    let own = || StatusCode::CONFLICT.into_response();
    let stalled = futures_util::stream::pending::<Result<Bytes, std::io::Error>>();
    let refusal = read_body_with(Body::from_stream(stalled), 8, own)
        .await
        .expect_err("a body that never finishes is refused");
    assert_eq!(refusal.status(), StatusCode::REQUEST_TIMEOUT);
    let refusal = read_body_with(Body::from(vec![0_u8; 9]), 8, own)
        .await
        .expect_err("a body past the cap is refused");
    assert_eq!(refusal.status(), StatusCode::CONFLICT);
    let body = read_body_with(Body::from("{}"), 8, own).await.unwrap();
    assert_eq!(&body[..], b"{}");
}

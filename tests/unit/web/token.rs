//! The `tests` module of `src/web/token.rs`, which declares this file by path.
//! Everything here reaches into `src/web/token.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use axum::http::{HeaderValue, header};

use super::*;

fn ip(last: u8) -> IpAddr {
    IpAddr::from([127, 0, 0, last])
}

fn forwarded(value: &str) -> header::HeaderMap {
    let mut headers = header::HeaderMap::new();
    headers.insert("x-forwarded-for", HeaderValue::from_str(value).unwrap());
    headers
}

fn peer() -> SocketAddr {
    SocketAddr::from(([10, 0, 0, 9], 4000))
}

#[test]
fn generated_candidate_identity_has_the_livekit_prefix() {
    let generated = generated_candidate_identity();
    assert!(generated.starts_with("candidate-"));
    assert_eq!(generated.len(), "candidate-".len() + CANDIDATE_SUFFIX_LEN);
}

#[test]
fn client_ip_ignores_forwarded_headers_until_a_proxy_is_declared() {
    // The default must not let a direct caller mint a new identity per request
    // and walk straight past the rate limit.
    assert_eq!(
        client_ip(&forwarded("1.2.3.4"), peer(), 0),
        peer().ip(),
        "a spoofed header must be ignored when no proxy is configured"
    );
    assert_eq!(
        client_ip(&header::HeaderMap::new(), peer(), 1),
        peer().ip(),
        "a configured proxy that sent no header falls back to the peer"
    );
}

/// Expiry is enforced on the entry that is read, not by pruning first, so
/// a grant the sweep has not reached yet must still be refused. The sweep
/// runs at most once per token lifetime, which is exactly long enough for
/// an expired grant to still be sitting in the map.
#[test]
fn an_observation_grant_expires_whether_or_not_the_sweep_has_run() {
    let mut rooms = RoomAuthorizations::default();
    rooms.grant("interview-one".to_string(), 7, 1_000);

    assert!(rooms.allows("interview-one", 7, 1_000));
    assert!(
        !rooms.allows("interview-one", 8, 1_000),
        "a grant belongs to the account it was made for"
    );
    assert!(!rooms.allows("interview-two", 7, 1_000));

    // One second past the token's life, and no second grant has run, so nothing
    // has swept.
    let expired = 1_000 + TOKEN_TTL_SECONDS + 1;
    assert!(!rooms.allows("interview-one", 7, expired));

    // A later grant sweeps the dead entry out rather than letting the map grow
    // for the life of the process.
    rooms.grant("interview-three".to_string(), 7, expired);
    assert_eq!(rooms.grants.len(), 1);
    assert!(rooms.allows("interview-three", 7, expired));

    // The last second a grant is good and the first it is not, and the sweep
    // read at the same edge: expiring on the second it runs is still expiring,
    // and a grant still inside its life is not swept out from under whoever is
    // watching with it.
    for (granted, trigger, left) in [(0, TOKEN_TTL_SECONDS, 1), (100, TOKEN_TTL_SECONDS + 50, 2)] {
        let mut rooms = RoomAuthorizations::default();
        rooms.grant("interview-first".to_string(), 1, granted);
        rooms.grant("interview-trigger".to_string(), 2, trigger);
        assert_eq!(
            rooms.grants.len(),
            left,
            "granted at {granted}, swept at {trigger}"
        );
    }

    let mut edge = RoomAuthorizations::default();
    edge.grant("interview-edge".to_string(), 7, 1_000);
    assert!(edge.allows("interview-edge", 7, 1_000 + TOKEN_TTL_SECONDS - 1));
    assert!(!edge.allows("interview-edge", 7, 1_000 + TOKEN_TTL_SECONDS));
}

#[test]
fn client_ip_reads_the_hop_the_operator_declared() {
    // Each proxy appends the address it saw, so the client sits `hops` from the
    // right; anything further left is caller-controlled.
    let chain = forwarded("9.9.9.9, 1.2.3.4, 172.16.0.1");
    assert_eq!(client_ip(&chain, peer(), 1), IpAddr::from([172, 16, 0, 1]));
    assert_eq!(client_ip(&chain, peer(), 2), IpAddr::from([1, 2, 3, 4]));
    assert_eq!(client_ip(&chain, peer(), 3), IpAddr::from([9, 9, 9, 9]));
    assert_eq!(
        client_ip(&chain, peer(), 4),
        peer().ip(),
        "more hops than entries means the chain is not what was declared"
    );
}

#[test]
fn client_ip_handles_ports_and_ipv6() {
    assert_eq!(
        client_ip(&forwarded("1.2.3.4:5678"), peer(), 1),
        IpAddr::from([1, 2, 3, 4])
    );
    assert_eq!(
        client_ip(&forwarded("2001:db8::1"), peer(), 1),
        "2001:db8::1".parse::<IpAddr>().unwrap(),
        "IPv6 colons must not be mistaken for a port"
    );
    assert_eq!(
        client_ip(&forwarded("[2001:db8::1]:443"), peer(), 1),
        "2001:db8::1".parse::<IpAddr>().unwrap()
    );
    assert_eq!(
        client_ip(&forwarded("not-an-address"), peer(), 1),
        peer().ip()
    );
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

/// A suffix is what separates one room from another, so the two things that
/// can go wrong with it are both silent. A `suffix` that stopped drawing
/// from the OS and returned anything fixed would put two candidates in one
/// room, each hearing the other's interview, and every caller would still
/// get a string of the right shape and length. An alphabet that quietly
/// changed would shrink the namespace the collision estimate rests on, or
/// widen it past what a LiveKit room name may hold.
///
/// Both are pinned by the character set the samples actually produce.
/// `DIGITS` is a `const` inside the function, so the expected alphabet is
/// written out here rather than read from the definition: sharing the
/// constant would let a changed alphabet agree with itself.
///
/// The sample size is chosen so the coverage assertion cannot flake. Each
/// draw misses a given character with probability 35/36, so over the ten
/// thousand characters below the chance any one of the thirty-six is
/// missing is under 10^-118. It is not a test of the distribution, only of
/// the reachable set.
#[test]
fn a_suffix_is_drawn_fresh_from_the_alphabet_it_documents() {
    const ALPHABET: &str = "0123456789abcdefghijklmnopqrstuvwxyz";

    let samples: Vec<String> = (0..500).map(|_| suffix(20)).collect();
    assert!(
        samples.iter().all(|sample| sample.chars().count() == 20),
        "the caller asked for a length and slices on it"
    );
    assert!(
        samples
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len()
            > 1,
        "every suffix was the same string, so rooms would collide by construction"
    );

    let mut seen: Vec<char> = samples
        .iter()
        .flat_map(|sample| sample.chars())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    seen.sort_unstable();
    assert_eq!(
        seen.iter().collect::<String>(),
        ALPHABET,
        "the alphabet drifted: a room name is lowercase alphanumeric and all of it is reachable"
    );
}

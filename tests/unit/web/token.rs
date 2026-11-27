//! The `tests` module of `src/web/token.rs`, which declares this file by path.
//! Everything here reaches into `src/web/token.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use std::net::IpAddr;

use axum::http::{HeaderValue, header};

use super::*;

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

/// A host holds a whole /64, so keying a limiter on the full address lets one
/// client mint a fresh bucket per request by rotating inside it.
#[test]
fn client_ip_charges_an_ipv6_client_by_its_prefix() {
    let one = client_ip(&forwarded("2001:db8:1:2::1"), peer(), 1);
    let rotated = client_ip(&forwarded("2001:db8:1:2:ffff:eeee:dddd:cccc"), peer(), 1);
    let neighbour = client_ip(&forwarded("2001:db8:1:3::1"), peer(), 1);
    assert_eq!(one, rotated, "one /64 is one client");
    assert_ne!(one, neighbour, "another /64 is another client");

    let v6_peer = "[2001:db8:1:2:aaaa::7]:443".parse().unwrap();
    assert_eq!(client_ip(&header::HeaderMap::new(), v6_peer, 0), one);
}

/// A dual-stack listener reports an IPv4 client as `::ffff:a.b.c.d`. Masked as
/// an IPv6 address that is `::` for everyone, which put every IPv4 client in
/// one bucket.
#[test]
fn client_ip_keeps_a_mapped_ipv4_client_apart() {
    let first = client_ip(&forwarded("::ffff:1.2.3.4"), peer(), 1);
    let second = client_ip(&forwarded("::ffff:5.6.7.8"), peer(), 1);
    assert_eq!(first, IpAddr::from([1, 2, 3, 4]));
    assert_ne!(first, second);

    let mapped_peer = "[::ffff:9.9.9.9]:443".parse().unwrap();
    assert_eq!(
        client_ip(&header::HeaderMap::new(), mapped_peer, 0),
        IpAddr::from([9, 9, 9, 9])
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
        "2001:db8::".parse::<IpAddr>().unwrap(),
        "IPv6 colons must not be mistaken for a port"
    );
    assert_eq!(
        client_ip(&forwarded("[2001:db8::1]:443"), peer(), 1),
        "2001:db8::".parse::<IpAddr>().unwrap()
    );
    assert_eq!(
        client_ip(&forwarded("not-an-address"), peer(), 1),
        peer().ip()
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

#[test]
fn random_style_accepts_the_last_complete_bucket_without_redrawing() {
    for (choices, byte, expected) in [
        (crate::agent::INTERVIEWER_VOICES, 255, "Orus"),
        (crate::agent::INTERVIEWER_ACCENTS, 254, "Indian"),
    ] {
        let mut draws = 0;
        let selected = resolve_interviewer_choice(Some(&json!("Random")), choices, || {
            draws += 1;
            if draws == 1 {
                Ok(byte)
            } else {
                Err("a valid boundary byte must not be rejected")
            }
        })
        .unwrap();
        assert_eq!(selected, Some(expected));
        assert_eq!(draws, 1);
    }
}

#[test]
fn random_style_draws_only_for_random_and_can_choose_every_option() {
    for (_, choices) in interviewer_choices() {
        for selection in [
            None,
            Some(json!("")),
            Some(json!("Default")),
            Some(json!(choices[0])),
        ] {
            let selected = resolve_interviewer_choice(selection.as_ref(), choices, || {
                panic!("explicit choices must not draw")
            })
            .unwrap();
            assert_eq!(
                selected,
                selection
                    .as_ref()
                    .and_then(|value| crate::agent::sanitize_interviewer_choice(
                        Some(value),
                        choices
                    ))
            );
        }
        for (index, expected) in choices.iter().enumerate() {
            let mut draws = 0;
            let selected = resolve_interviewer_choice(Some(&json!("Random")), choices, || {
                draws += 1;
                Ok(index as u8)
            })
            .unwrap();
            assert_eq!(selected, Some(*expected));
            assert_eq!(draws, 1);
        }
    }
    let mut draws = [255, 4].into_iter();
    assert_eq!(
        resolve_interviewer_choice(
            Some(&json!("Random")),
            crate::agent::INTERVIEWER_ACCENTS,
            || Ok(draws.next().unwrap())
        )
        .unwrap(),
        Some("Indian")
    );
    assert!(
        draws.next().is_none(),
        "the incomplete bucket must be rejected"
    );
    assert!(
        resolve_interviewer_choice(
            Some(&json!("Random")),
            crate::agent::INTERVIEWER_VOICES,
            || Err("entropy unavailable")
        )
        .is_err()
    );
}

/// A proxy may add its own `X-Forwarded-For` line instead of appending to the
/// client's, and HTTP reads several lines as one list, in order. The hop
/// counted from the right has to come from the last line, the proxy's, and not
/// from whichever line the client sent first.
#[test]
fn the_trusted_hop_is_counted_across_forwarded_header_lines() {
    let mut headers = header::HeaderMap::new();
    headers.append("x-forwarded-for", HeaderValue::from_static("203.0.113.7"));
    headers.append("x-forwarded-for", HeaderValue::from_static("198.51.100.4"));
    assert_eq!(
        client_ip(&headers, peer(), 1),
        IpAddr::from([198, 51, 100, 4])
    );
    assert_eq!(
        client_ip(&headers, peer(), 2),
        IpAddr::from([203, 0, 113, 7])
    );
}

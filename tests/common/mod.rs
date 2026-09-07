//! What the integration tests share, which today is one predicate.
//!
//! `tests/common/mod.rs` rather than `tests/common.rs`, because Cargo compiles
//! every `tests/*.rs` as its own test binary and a bare `common.rs` would
//! become a target with no tests in it.

/// Whether a bearer token is one a given LiveKit project would issue: its `iss`
/// is that project's api key, and its signature verifies under that project's
/// secret.
///
/// Shared because two integration tests now stand a double in front of the same
/// credential and have to make the same judgement about it. `tests/web.rs` puts
/// one in front of the pool's quota probe, `tests/cli.rs` in front of the Setup
/// page's, and both exist because a double that answers its status to any
/// caller leaves what production sends as the one thing no test looks at. A
/// second copy of that judgement is a second place for it to get weaker
/// quietly.
///
/// Only the judgement is here. Getting the token out stays with each caller:
/// one reads an `axum` `HeaderMap`, the other a raw request head off a
/// `TcpStream`, and nothing is gained by teaching this function about either.
///
/// Two more copies live in the unit tests that `src/web/pool.rs` and
/// `src/livekit/rooms.rs` declare, and they stay copies by choice rather than
/// by necessity. The necessity argument used to be written here and was wrong:
/// it said a unit test compiles under `--cfg test` while an integration test
/// links the plain rlib, so the two halves cannot see one module. They cannot
/// share a compiled module, but they can share this source -- the same
/// `#[path]`
/// that moved every unit test body under `tests/` would include this file too,
/// needing only `extern crate self as codetrial;` in `src/lib.rs` for the paths
/// below to resolve on the inside.
///
/// What actually keeps them apart is the trade `src/livekit/rooms.rs` states:
/// three lines are common, while each site's token source and follow-on check
/// are not. This copy exists because two integration tests in the same position
/// needed the same judgement; that reason does not reach a unit test, and the
/// day it does, the mechanism is available rather than forbidden.
///
/// The issuer and the signature, and nothing else: not the algorithm header,
/// the expiry, or the grant. That is the whole claim a caller may make of it,
/// that this token is this project's. It is not an emulation of LiveKit.
pub fn livekit_token_matches(token: &str, api_key: &str, api_secret: &str) -> bool {
    if codetrial::token::livekit_token_issuer(token).as_deref() != Some(api_key) {
        return false;
    }

    // `rsplit_once` splits a four-segment string as happily as a three-segment
    // one, so a structural check belongs somewhere -- but it is already above.
    // `livekit_token_issuer` takes the payload as everything between the first
    // dot and the last, and base64url has no dot in it, so a token with a
    // segment glued on fails to decode there and never reaches this line. A
    // check here would be a line no test could reach; both callers ask for the
    // four-segment case instead, so that a parser which stopped refusing it is
    // a failure rather than a silent widening.
    let Some((signing_input, signature)) = token.rsplit_once('.') else {
        return false;
    };
    codetrial::token::verify_hs256(api_secret, signing_input, signature)
}

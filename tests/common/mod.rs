//! What the integration tests share, which today is one predicate.
//!
//! `tests/common/mod.rs` rather than `tests/common.rs`, because Cargo compiles
//! every `tests/*.rs` as its own test binary and a bare `common.rs` would
//! become a target with no tests in it.

/// Whether a bearer token is one a given LiveKit project would issue: its `iss`
/// is that project's api key, and its signature verifies under that project's
/// secret.
///
/// Its own module because it is a judgement rather than a stub: `tests/web.rs`
/// stands a double in front of the pool's quota probe, and what makes a token
/// that project's is the part of that double worth stating once. The double
/// exists because one that answers its status to any caller leaves the
/// credential production sends as the one thing no test looks at, so a second
/// copy of the judgement would be a second place for it to get weaker quietly.
///
/// Only the judgement is here. Getting the token out stays with the caller,
/// which reads an `axum` `HeaderMap`, and nothing is gained by teaching this
/// function about where a token was found.
///
/// Two more copies live in `src/web/pool.rs` and `src/livekit/rooms.rs`, and
/// neither can use this one. A unit test compiles against the crate under
/// `--cfg test` while an integration test links the plain rlib, so those two
/// halves of the tree cannot see one module. `src/livekit/rooms.rs` carries the
/// argument for why that pair stays a pair.
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
    // check here would be a line no test could reach; the caller asks for the
    // four-segment case instead, so that a parser which stopped refusing it is
    // a failure rather than a silent widening.
    let Some((signing_input, signature)) = token.rsplit_once('.') else {
        return false;
    };
    codetrial::token::verify_hs256(api_secret, signing_input, signature)
}

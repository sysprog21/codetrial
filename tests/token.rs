//! The credential boundary. Everything here is a property the module already
//! holds and states in a comment, pinned so that a later edit has to argue with
//! a failing test rather than with prose.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use codetrial::token::{
    LivekitTokenInput, TOKEN_TTL_SECONDS, WebhookRejection, livekit_egress_token,
    livekit_observer_token, livekit_room_admin_token, livekit_room_from_token, livekit_token,
    livekit_token_issuer, sign_hs256, verify_hs256, verify_livekit_webhook,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

const KEY: &str = "APIkey123";
const SECRET: &str = "a-secret-long-enough-to-look-real";
const NOW: u64 = 1_700_000_000;

/// The claims a token carries, read without verifying. Tests assert on grants,
/// and reading them back out of the wire form is the only way to be sure the
/// grant that was written is the grant that was signed.
fn claims_of(token: &str) -> Value {
    let payload = token.split('.').nth(1).expect("a JWT has three segments");
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap()
}

/// A JWT with whatever claims and signature the caller wants. Forgeries have to
/// be built by hand: every minting function in the module signs correctly, so
/// none of them can produce the inputs the rejection paths exist for.
fn forge(header: &Value, claims: &Value, signature: &str) -> String {
    let head = URL_SAFE_NO_PAD.encode(serde_json::to_vec(header).unwrap());
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
    format!("{head}.{body}.{signature}")
}

fn webhook_claims(body: &[u8], expires_at: u64) -> Value {
    json!({
        "iss": KEY,
        "exp": expires_at,
        "nbf": NOW,
        "sha256": base64::engine::general_purpose::STANDARD.encode(Sha256::digest(body)),
    })
}

/// Signs `claims` the way LiveKit would, so the happy path under test is the
/// real wire format and not a shape these tests invented.
fn signed_webhook(claims: &Value, secret: &str) -> String {
    let head =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"alg":"HS256","typ":"JWT"})).unwrap());
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap());
    let signing_input = format!("{head}.{body}");
    let signature = sign_hs256(secret, &signing_input);
    format!("{signing_input}.{signature}")
}

#[test]
fn a_signature_verifies_only_over_the_bytes_and_secret_that_made_it() {
    let signature = sign_hs256(SECRET, "session=42");
    assert!(verify_hs256(SECRET, "session=42", &signature));

    // The three ways this must say no. A verifier that answers `true` to any of
    // them is a forgeable session cookie, and none of them look wrong at the
    // call site.
    assert!(
        !verify_hs256(SECRET, "session=43", &signature),
        "a different value must not verify"
    );
    assert!(
        !verify_hs256("another-secret", "session=42", &signature),
        "a different secret must not verify"
    );
    assert!(
        !verify_hs256(SECRET, "session=42", &sign_hs256(SECRET, "session=43")),
        "a signature over other bytes must not verify"
    );
}

#[test]
fn a_signature_that_is_not_base64_is_refused_rather_than_panicking() {
    for signature in ["", "!!!!", "not base64", "AAAA"] {
        assert!(
            !verify_hs256(SECRET, "session=42", signature),
            "{signature:?} must not verify"
        );
    }
}

/// The module says a publish-capable observer is unrepresentable at its call
/// site. That is only true while the grants stay as written, and a grant is a
/// JSON literal that no compiler checks.
#[test]
fn each_token_carries_only_the_grant_its_purpose_needs() {
    let candidate = claims_of(
        &livekit_token(LivekitTokenInput {
            api_key: KEY,
            api_secret: SECRET,
            name: "Ada",
            identity: "candidate-1",
            room: "interview-a1b2c3d4",
            metadata: "{}",
            now_seconds: NOW,
            agent: false,
        })
        .unwrap(),
    );
    assert_eq!(candidate["video"]["canPublish"], json!(true));
    assert_eq!(candidate["video"]["agent"], json!(false));
    assert_eq!(candidate["video"]["roomAdmin"], Value::Null);
    assert_eq!(candidate["video"]["roomRecord"], Value::Null);
    assert_eq!(candidate["kind"], Value::Null, "only an agent is a kind");

    let observer = claims_of(
        &livekit_observer_token(KEY, SECRET, "observer-1", "interview-a1b2c3d4", NOW).unwrap(),
    );
    assert_eq!(observer["video"]["canPublish"], json!(false));
    assert_eq!(observer["video"]["canPublishData"], json!(false));
    assert_eq!(observer["video"]["canSubscribe"], json!(true));
    assert_eq!(observer["video"]["roomAdmin"], Value::Null);

    // `roomAdmin` can mutate a room, `roomRecord` cannot. Neither may do the
    // other's job, which is the whole reason they are two functions.
    let admin =
        claims_of(&livekit_room_admin_token(KEY, SECRET, "interview-a1b2c3d4", NOW).unwrap());
    assert_eq!(admin["video"]["roomAdmin"], json!(true));
    assert_eq!(admin["video"]["roomRecord"], Value::Null);
    assert_eq!(admin["video"]["roomJoin"], Value::Null);

    let egress = claims_of(&livekit_egress_token(KEY, SECRET, "interview-a1b2c3d4", NOW).unwrap());
    assert_eq!(egress["video"]["roomRecord"], json!(true));
    assert_eq!(egress["video"]["roomAdmin"], Value::Null);
}

#[test]
fn every_token_expires() {
    let tokens = [
        livekit_token(LivekitTokenInput {
            api_key: KEY,
            api_secret: SECRET,
            name: "Ada",
            identity: "candidate-1",
            room: "interview-a1b2c3d4",
            metadata: "{}",
            now_seconds: NOW,
            agent: true,
        })
        .unwrap(),
        livekit_observer_token(KEY, SECRET, "observer-1", "interview-a1b2c3d4", NOW).unwrap(),
        livekit_room_admin_token(KEY, SECRET, "interview-a1b2c3d4", NOW).unwrap(),
        livekit_egress_token(KEY, SECRET, "interview-a1b2c3d4", NOW).unwrap(),
    ];
    for token in tokens {
        let claims = claims_of(&token);
        assert_eq!(claims["nbf"], json!(NOW));
        assert_eq!(
            claims["exp"],
            json!(NOW + TOKEN_TTL_SECONDS),
            "a token without an expiry is a permanent credential"
        );
    }
}

#[test]
fn the_issuer_is_readable_before_anything_is_verified() {
    let token = livekit_room_admin_token(KEY, SECRET, "interview-a1b2c3d4", NOW).unwrap();
    assert_eq!(livekit_token_issuer(&token).as_deref(), Some(KEY));

    // It selects a secret, so it must decline rather than guess.
    assert_eq!(livekit_token_issuer("not-a-jwt"), None);
    assert_eq!(livekit_token_issuer("only.two"), None);
    assert_eq!(
        livekit_token_issuer(&forge(&json!({}), &json!({"iss": ""}), "x")),
        None,
        "an empty issuer names no project"
    );
}

#[test]
fn a_recorder_token_proves_its_room_and_nothing_else_does() {
    let token = livekit_token(LivekitTokenInput {
        api_key: KEY,
        api_secret: SECRET,
        name: "Recorder",
        identity: "egress",
        room: "interview-eu-west-a1b2c3d4",
        metadata: "{}",
        now_seconds: NOW,
        agent: false,
    })
    .unwrap();
    assert_eq!(
        livekit_room_from_token(KEY, SECRET, &token, NOW + 60).unwrap(),
        "interview-eu-west-a1b2c3d4"
    );

    // Signed by another project, so this server's secret must not vouch for it.
    assert_eq!(
        livekit_room_from_token(KEY, "other-secret", &token, NOW + 60),
        Err(WebhookRejection::BadSignature)
    );
    assert_eq!(
        livekit_room_from_token("OTHERkey", SECRET, &token, NOW + 60),
        Err(WebhookRejection::UnknownKey)
    );
    assert_eq!(
        livekit_room_from_token(KEY, SECRET, &token, NOW + TOKEN_TTL_SECONDS),
        Err(WebhookRejection::Expired)
    );

    // A credential good for something other than joining is not a recorder's,
    // whatever room it names.
    let admin = livekit_room_admin_token(KEY, SECRET, "interview-a1b2c3d4", NOW).unwrap();
    assert_eq!(
        livekit_room_from_token(KEY, SECRET, &admin, NOW + 60),
        Err(WebhookRejection::Malformed),
        "roomAdmin without roomJoin must not pass as a recorder"
    );
}

#[test]
fn a_webhook_is_accepted_only_when_every_check_holds() {
    let body = br#"{"event":"room_finished"}"#;
    let good = signed_webhook(&webhook_claims(body, NOW + 300), SECRET);
    assert_eq!(
        verify_livekit_webhook(KEY, SECRET, &good, body, NOW),
        Ok(())
    );

    let cases: [(&str, String, &[u8], u64, WebhookRejection); 6] = [
        (
            "not a jwt",
            "nodots".to_string(),
            body,
            NOW,
            WebhookRejection::Malformed,
        ),
        (
            "signed by another project",
            signed_webhook(&webhook_claims(body, NOW + 300), "other-secret"),
            body,
            NOW,
            WebhookRejection::BadSignature,
        ),
        (
            "past its expiry",
            signed_webhook(&webhook_claims(body, NOW + 300), SECRET),
            body,
            NOW + 300,
            WebhookRejection::Expired,
        ),
        (
            "not yet valid",
            signed_webhook(&webhook_claims(body, NOW + 300), SECRET),
            body,
            NOW - 1,
            WebhookRejection::Expired,
        ),
        (
            "a valid signature over a different body",
            signed_webhook(&webhook_claims(body, NOW + 300), SECRET),
            b"{\"event\":\"room_started\"}",
            NOW,
            WebhookRejection::BodyMismatch,
        ),
        (
            "no expiry at all",
            signed_webhook(&json!({"iss": KEY, "nbf": NOW}), SECRET),
            body,
            NOW,
            WebhookRejection::Malformed,
        ),
    ];
    for (label, authorization, body, now, expected) in cases {
        assert_eq!(
            verify_livekit_webhook(KEY, SECRET, &authorization, body, now),
            Err(expected),
            "{label}"
        );
    }

    // A key this server does not serve is refused before the secret is used.
    assert_eq!(
        verify_livekit_webhook("OTHERkey", SECRET, &good, body, NOW),
        Err(WebhookRejection::UnknownKey)
    );
}

/// The module declines to read the JWT header, on the grounds that always
/// computing HMAC-SHA256 makes algorithm confusion impossible. That reasoning
/// is only sound while the verifier really does ignore `alg`, so both halves
/// are pinned: `none` is refused, and a header claiming something else is
/// irrelevant to a token that is otherwise correct.
#[test]
fn the_algorithm_a_token_claims_changes_nothing() {
    let body = br#"{"event":"room_finished"}"#;
    let claims = webhook_claims(body, NOW + 300);

    let unsigned = forge(&json!({"alg": "none", "typ": "JWT"}), &claims, "");
    assert_eq!(
        verify_livekit_webhook(KEY, SECRET, &unsigned, body, NOW),
        Err(WebhookRejection::BadSignature),
        "an unsigned token must not be trusted for saying it needs no signature"
    );

    // Correctly HMAC-signed, but the header lies about how. It is accepted,
    // because the header was never consulted.
    let head = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({"alg":"RS256"})).unwrap());
    let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    let signing_input = format!("{head}.{payload}");
    let mislabelled = format!("{signing_input}.{}", sign_hs256(SECRET, &signing_input));
    assert_eq!(
        verify_livekit_webhook(KEY, SECRET, &mislabelled, body, NOW),
        Ok(())
    );
}

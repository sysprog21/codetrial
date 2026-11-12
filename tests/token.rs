#![expect(
    clippy::unwrap_used,
    reason = "a test that fails by panicking is working as intended"
)]

//! The credential boundary. Everything here is a property the module already
//! holds and states in a comment, pinned so that a later edit has to argue with
//! a failing test rather than with prose.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use codetrial::token::{
    LivekitTokenInput, TOKEN_TTL_SECONDS, WebhookRejection, livekit_egress_token,
    livekit_join_from_token, livekit_observer_token, livekit_room_admin_token,
    livekit_room_create_token, livekit_room_from_token, livekit_room_list_token, livekit_token,
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
    assert_eq!(candidate["name"], json!("Ada"));
    assert_eq!(candidate["metadata"], json!("{}"));
    assert_eq!(candidate["video"]["canPublish"], json!(true));
    assert_eq!(candidate["video"]["agent"], json!(false));
    assert_eq!(candidate["video"]["roomAdmin"], Value::Null);
    assert_eq!(candidate["video"]["roomRecord"], Value::Null);
    assert_eq!(candidate["kind"], Value::Null, "only an agent is a kind");

    let observer = claims_of(
        &livekit_observer_token(KEY, SECRET, "observer-1", "interview-a1b2c3d4", NOW).unwrap(),
    );
    assert_eq!(observer["iss"], json!(KEY));
    assert_eq!(observer["name"], json!("Interviewer observer"));
    assert_eq!(observer["video"]["canPublish"], json!(false));
    assert_eq!(observer["video"]["canPublishData"], json!(false));
    assert_eq!(observer["video"]["canSubscribe"], json!(true));
    assert_eq!(observer["video"]["roomAdmin"], Value::Null);

    // Metadata is how a participant announces itself to the room, and the
    // browser reads the interviewer's. An observer that can rewrite its own is
    // a listener that can still say things, which is the one thing the
    // publish-free grants above exist to prevent.
    assert_eq!(observer["video"]["canUpdateOwnMetadata"], json!(false));

    // `roomAdmin` can mutate a room, `roomRecord` cannot. Neither may do the
    // other's job, which is the whole reason they are two functions.
    let admin =
        claims_of(&livekit_room_admin_token(KEY, SECRET, "interview-a1b2c3d4", NOW).unwrap());
    assert_eq!(admin["video"]["roomAdmin"], json!(true));
    assert_eq!(admin["video"]["roomRecord"], Value::Null);
    assert_eq!(admin["video"]["roomJoin"], Value::Null);

    let egress = claims_of(&livekit_egress_token(KEY, SECRET, "interview-a1b2c3d4", NOW).unwrap());
    assert_eq!(egress["iss"], json!(KEY));
    assert_eq!(egress["video"]["roomRecord"], json!(true));
    assert_eq!(egress["video"]["roomAdmin"], Value::Null);

    // The room, because "room-scoped" is the whole claim this minter makes over
    // a project-wide recording credential, and a grant that dropped the name
    // would record every interview in the project.
    assert_eq!(egress["video"]["room"], json!("interview-a1b2c3d4"));

    // Named no room, and cannot be given one after the fact: the Setup page
    // signs this with credentials nobody has verified yet, only to learn from
    // LiveKit whether they are real. Listing is the least it can ask for and
    // still get an answer.
    let list = claims_of(&livekit_room_list_token(KEY, SECRET, NOW).unwrap());
    assert_eq!(list["iss"], json!(KEY));
    assert_eq!(list["video"]["roomList"], json!(true));
    assert_eq!(list["video"]["room"], Value::Null);
    assert_eq!(list["video"]["roomJoin"], Value::Null);
    assert_eq!(list["video"]["roomAdmin"], Value::Null);
    assert_eq!(list["video"]["roomRecord"], Value::Null);

    // DeleteRoom is gated on `roomCreate`, and room cleanup needs nothing else:
    // not the `roomAdmin` that evicts participants, not a seat in the room.
    let create = claims_of(&livekit_room_create_token(KEY, SECRET, NOW).unwrap());
    assert_eq!(create["iss"], json!(KEY));
    assert_eq!(create["video"]["roomCreate"], json!(true));
    assert_eq!(create["video"]["room"], Value::Null);
    assert_eq!(create["video"]["roomAdmin"], Value::Null);
    assert_eq!(create["video"]["roomJoin"], Value::Null);
    assert_eq!(create["video"]["roomList"], Value::Null);
    assert_eq!(create["video"]["roomRecord"], Value::Null);
}

#[test]
fn every_token_expires() {
    // The interview credentials last as long as an interview, and the
    // management ones are minted for one call, so five minutes.
    for now_seconds in [NOW, u64::MAX - TOKEN_TTL_SECONDS] {
        let tokens = [
            livekit_token(LivekitTokenInput {
                api_key: KEY,
                api_secret: SECRET,
                name: "Ada",
                identity: "candidate-1",
                room: "interview-a1b2c3d4",
                metadata: "{}",
                now_seconds,
                agent: true,
            })
            .unwrap(),
            livekit_observer_token(KEY, SECRET, "observer-1", "interview-a1b2c3d4", now_seconds)
                .unwrap(),
            livekit_room_admin_token(KEY, SECRET, "interview-a1b2c3d4", now_seconds).unwrap(),
            livekit_egress_token(KEY, SECRET, "interview-a1b2c3d4", now_seconds).unwrap(),
            livekit_room_list_token(KEY, SECRET, now_seconds).unwrap(),
            livekit_room_create_token(KEY, SECRET, now_seconds).unwrap(),
        ];
        for (token, (identity, ttl)) in tokens.into_iter().zip([
            ("candidate-1", TOKEN_TTL_SECONDS),
            ("observer-1", TOKEN_TTL_SECONDS),
            ("room-admin", 300),
            ("recording", 300),
            ("credential-check", 300),
            ("room-cleanup", 300),
        ]) {
            let (signing_input, signature) = token.rsplit_once('.').unwrap();
            assert!(verify_hs256(SECRET, signing_input, signature));
            let claims = claims_of(&token);
            assert_eq!(claims["iss"], json!(KEY));
            assert_eq!(claims["sub"], json!(identity));
            assert_eq!(claims["nbf"], json!(now_seconds));
            assert_eq!(
                claims["exp"],
                json!(now_seconds + ttl),
                "a token without an expiry is a permanent credential"
            );
        }
    }
}

/// A clock too close to the top of `u64` must fail, not mint a token whose
/// expiry saturates into a credential that never lapses.
#[test]
fn a_token_whose_expiry_overflows_is_refused() {
    let management = u64::MAX - 300 + 1;
    assert!(livekit_room_list_token(KEY, SECRET, management).is_err());
    assert!(livekit_room_create_token(KEY, SECRET, management).is_err());
    assert!(livekit_room_admin_token(KEY, SECRET, "r", management).is_err());
    assert!(livekit_egress_token(KEY, SECRET, "r", management).is_err());
    let now_seconds = u64::MAX - TOKEN_TTL_SECONDS + 1;
    assert!(livekit_observer_token(KEY, SECRET, "o", "r", now_seconds).is_err());
    assert!(
        livekit_token(LivekitTokenInput {
            api_key: KEY,
            api_secret: SECRET,
            name: "Ada",
            identity: "candidate-1",
            room: "r",
            metadata: "{}",
            now_seconds,
            agent: false,
        })
        .is_err()
    );
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

    // A joinable token naming no room proves nothing. The caller compares the
    // answer against the room it is serving, so an empty string returned as a
    // room name is a comparison against a value no interview can hold, and the
    // refusal has to happen here rather than being left to that comparison.
    let unnamed = livekit_token(LivekitTokenInput {
        api_key: KEY,
        api_secret: SECRET,
        name: "Recorder",
        identity: "egress",
        room: "",
        metadata: "{}",
        now_seconds: NOW,
        agent: false,
    })
    .unwrap();
    assert_eq!(
        livekit_room_from_token(KEY, SECRET, &unnamed, NOW + 60),
        Err(WebhookRejection::Malformed),
        "an empty room name is not a room"
    );
}

/// A release names its start by the identity its token joins as, so that
/// identity has to come out of the signed claims and nowhere else, and a
/// token that names none releases nothing.
#[test]
fn a_join_token_proves_its_room_and_identity() {
    let token = livekit_token(LivekitTokenInput {
        api_key: KEY,
        api_secret: SECRET,
        name: "Candidate",
        identity: "candidate-a1b2c3",
        room: "interview-a1b2c3d4",
        metadata: "{}",
        now_seconds: NOW,
        agent: false,
    })
    .unwrap();
    assert_eq!(
        livekit_join_from_token(KEY, SECRET, &token, NOW + 60).unwrap(),
        (
            "interview-a1b2c3d4".to_string(),
            "candidate-a1b2c3".to_string()
        )
    );
    assert_eq!(
        livekit_join_from_token(KEY, "other-secret", &token, NOW + 60),
        Err(WebhookRejection::BadSignature)
    );
    assert_eq!(
        livekit_join_from_token(KEY, SECRET, &token, NOW + TOKEN_TTL_SECONDS),
        Err(WebhookRejection::Expired)
    );

    let admin = livekit_room_admin_token(KEY, SECRET, "interview-a1b2c3d4", NOW).unwrap();
    assert_eq!(
        livekit_join_from_token(KEY, SECRET, &admin, NOW + 60),
        Err(WebhookRejection::Malformed),
        "roomAdmin without roomJoin must not pass as a join"
    );

    // Signed correctly, so the refusal is the missing identity and not the
    // signature check passing for it.
    let mut claims = claims_of(&token);
    claims.as_object_mut().unwrap().remove("sub");
    let (head, _) = token.split_once('.').unwrap();
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
    let input = format!("{head}.{body}");
    let anonymous = format!("{input}.{}", sign_hs256(SECRET, &input));
    assert_eq!(
        livekit_join_from_token(KEY, SECRET, &anonymous, NOW + 60),
        Err(WebhookRejection::Malformed),
        "a join token naming no identity cannot say whose start it is"
    );
}

/// The cross-purpose direction the module states nowhere: every credential in
/// this file is signed with the same secret, so nothing about a token's
/// signature says which door it was minted for. A candidate's own join token
/// has the right `iss`, verifies under this server's secret and sits inside its
/// validity window -- every check a webhook passes except the last one. The
/// body digest is the only thing between it and a forged `room_finished`, which
/// is why LiveKit puts the digest inside the token rather than trusting the
/// signature alone.
#[test]
fn a_join_token_is_not_a_webhook_signature() {
    let body = br#"{"event":"room_finished"}"#;
    let join = livekit_token(LivekitTokenInput {
        api_key: KEY,
        api_secret: SECRET,
        name: "Ada",
        identity: "candidate-1",
        room: "interview-a1b2c3d4",
        metadata: "{}",
        now_seconds: NOW,
        agent: false,
    })
    .unwrap();

    // Everything before the digest accepts it, so the refusal below is the
    // digest check and cannot be an earlier one passing for it.
    assert_eq!(
        livekit_room_from_token(KEY, SECRET, &join, NOW + 60).unwrap(),
        "interview-a1b2c3d4"
    );
    assert_eq!(
        verify_livekit_webhook(KEY, SECRET, &join, body, NOW + 60),
        Err(WebhookRejection::BodyMismatch),
        "a token carrying no digest must not vouch for a body"
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

#[test]
fn hs256_matches_rfc4231_and_independent_key_length_cases() {
    use hmac::{Hmac, KeyInit, Mac};
    let key = "\x0b".repeat(20);
    let signature = "sDRMYdjbOFNcqK_OrwvxK4gdwgDJgz2nJuk3bC4yz_c";
    assert_eq!(sign_hs256(&key, "Hi There"), signature);
    assert!(verify_hs256(&key, "Hi There", signature));
    for key in [String::new(), "k".repeat(64), "k".repeat(131)] {
        let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).unwrap();
        mac.update(b"payload");
        let expected = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        assert_eq!(sign_hs256(&key, "payload"), expected);
        assert!(verify_hs256(&key, "payload", &expected));
        let truncated = URL_SAFE_NO_PAD.encode(&URL_SAFE_NO_PAD.decode(&expected).unwrap()[..31]);
        assert!(!verify_hs256(&key, "payload", &truncated));
    }
}

#[test]
fn peer_tokens_are_short_lived_and_cannot_administer_rooms() {
    let token = codetrial::token::livekit_peer_token(LivekitTokenInput {
        api_key: KEY,
        api_secret: SECRET,
        name: "Partner",
        identity: "opaque",
        room: "peer-room",
        metadata: "",
        now_seconds: NOW,
        agent: true,
    })
    .unwrap();
    assert_eq!(
        livekit_join_from_token(KEY, SECRET, &token, NOW + 299).unwrap(),
        ("peer-room".into(), "opaque".into())
    );
    assert_eq!(
        livekit_join_from_token(KEY, SECRET, &token, NOW + 300),
        Err(WebhookRejection::Expired)
    );
    let claims = claims_of(&token);
    assert_eq!(claims["exp"], NOW + 300);
    assert_eq!(claims["nbf"], NOW);
    assert_eq!(claims["sub"], "opaque");
    assert_eq!(claims["video"]["room"], "peer-room");
    assert_eq!(claims["video"]["agent"], false);
    assert_eq!(claims["video"]["canUpdateOwnMetadata"], false);
    for grant in ["roomAdmin", "roomCreate", "roomRecord"] {
        assert!(claims["video"].get(grant).is_none());
    }
    assert!(claims.get("kind").is_none());
    assert!(
        codetrial::token::livekit_peer_token(LivekitTokenInput {
            api_key: KEY,
            api_secret: SECRET,
            name: "Partner",
            identity: "opaque",
            room: "peer-room",
            metadata: "",
            now_seconds: u64::MAX,
            agent: false,
        })
        .is_err()
    );
}

#[test]
fn peer_observer_token_cannot_publish_any_room_content() {
    let token =
        codetrial::token::livekit_peer_observer_token(KEY, SECRET, "observer", "peer-room", NOW)
            .unwrap();
    let claims = claims_of(&token);
    assert_eq!(claims["kind"], "agent");
    assert_eq!(
        claims["video"],
        json!({"room":"peer-room","roomJoin":true,"canPublish":false,"canSubscribe":true,"canPublishData":false,"canUpdateOwnMetadata":false,"agent":true})
    );
    assert_eq!(
        livekit_join_from_token(KEY, SECRET, &token, NOW).unwrap(),
        ("peer-room".into(), "observer".into())
    );
}

/// Each management credential is minted for one call, and a leaked one can act
/// project-wide (the room-create token deletes any room), so none of them
/// lives as long as an interview: five minutes, as the peer token.
#[test]
fn management_credentials_are_short_lived() {
    use codetrial::token::{
        livekit_egress_token, livekit_room_admin_token, livekit_room_create_token,
        livekit_room_list_token,
    };
    let tokens = [
        livekit_room_admin_token(KEY, SECRET, "room", NOW).unwrap(),
        livekit_room_list_token(KEY, SECRET, NOW).unwrap(),
        livekit_room_create_token(KEY, SECRET, NOW).unwrap(),
        livekit_egress_token(KEY, SECRET, "room", NOW).unwrap(),
    ];
    for token in tokens {
        assert_eq!(claims_of(&token)["exp"], NOW + 300);
    }
}

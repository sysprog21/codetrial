//! LiveKit access tokens, and the one credential that arrives rather than
//! leaves: the JWT LiveKit signs its webhooks with.
//!
//! Both the HTTP layer (candidate tokens) and the agent runtime (join and room
//! admin tokens) mint these, so the signing lives here rather than in either
//! caller. Verification lives here for the same reason: it is the same
//! algorithm read backwards, and a second copy of it elsewhere would be a
//! second chance to accept something this one refuses.

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use ring::{digest, hmac};
use serde_json::{Value, json};

/// HMAC-SHA256 over `value`, base64url without padding.
///
/// The one place that knows the algorithm. The JWT signer below and the signed
/// cookies in `web` both go through it, because two copies of a signing step
/// are two chances for one of them to drift into something weaker.
pub fn sign_hs256(secret: &str, value: &str) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    URL_SAFE_NO_PAD.encode(hmac::sign(&key, value.as_bytes()).as_ref())
}

/// Whether `signature` is `sign_hs256(secret, value)`.
///
/// The comparison is `ring::hmac::verify`, which is constant time. A byte-wise
/// `==` on the base64 would leak how much of a forged signature was right.
pub fn verify_hs256(secret: &str, value: &str, signature: &str) -> bool {
    let Ok(signature) = URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    hmac::verify(&key, value.as_bytes(), &signature).is_ok()
}

pub const TOKEN_TTL_SECONDS: u64 = 2 * 60 * 60;

/// No `Debug`: `api_secret` is the key every token in this file is signed
/// with, so printing one instance of this leaks more than any single token.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LivekitTokenInput<'a> {
    pub api_key: &'a str,
    pub api_secret: &'a str,
    pub name: &'a str,
    pub identity: &'a str,
    pub room: &'a str,
    pub metadata: &'a str,
    pub now_seconds: u64,
    pub agent: bool,
}

pub fn livekit_token(
    input: LivekitTokenInput<'_>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut claims = livekit_claims(
        input.api_key,
        input.identity,
        input.now_seconds,
        json!({
            "room": input.room,
            "roomJoin": true,
            "canPublish": true,
            "canSubscribe": true,
            "canPublishData": true,
            "agent": input.agent,
            "canUpdateOwnMetadata": input.agent
        }),
    )?;
    claims["name"] = json!(input.name);
    claims["metadata"] = json!(input.metadata);
    if input.agent {
        claims["kind"] = json!("agent");
    }
    sign_jwt(input.api_secret, &claims)
}

/// A five-minute join credential for one fixed peer participant.
pub fn livekit_peer_token(
    input: LivekitTokenInput<'_>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut claims = livekit_claims_with_ttl(
        input.api_key,
        input.identity,
        input.now_seconds,
        300,
        json!({ "room": input.room, "roomJoin": true, "canPublish": true,
            "canSubscribe": true, "canPublishData": true, "agent": false,
            "canUpdateOwnMetadata": false }),
    )?;
    claims["name"] = json!(input.name);
    claims["metadata"] = json!(input.metadata);
    sign_jwt(input.api_secret, &claims)
}

/// The peer observer can subscribe but cannot publish audio, data or metadata.
pub fn livekit_peer_observer_token(
    api_key: &str,
    api_secret: &str,
    identity: &str,
    room: &str,
    now_seconds: u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut claims = livekit_claims(
        api_key,
        identity,
        now_seconds,
        json!({
            "room":room,"roomJoin":true,"canPublish":false,"canSubscribe":true,
            "canPublishData":false,"canUpdateOwnMetadata":false,"agent":true,
        }),
    )?;
    claims["name"] = json!("Silent AI observer");
    claims["kind"] = json!("agent");
    sign_jwt(api_secret, &claims)
}

/// A listen-only room credential for the interviewer. Keeping this separate
/// from `livekit_token` makes a publish-capable observer unrepresentable at
/// its call site.
pub fn livekit_observer_token(
    api_key: &str,
    api_secret: &str,
    identity: &str,
    room: &str,
    now_seconds: u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let mut claims = livekit_claims(
        api_key,
        identity,
        now_seconds,
        json!({
            "room": room,
            "roomJoin": true,
            "canPublish": false,
            "canSubscribe": true,
            "canPublishData": false,
            "canUpdateOwnMetadata": false
        }),
    )?;
    claims["name"] = json!("Interviewer observer");
    sign_jwt(api_secret, &claims)
}

pub fn livekit_room_admin_token(
    api_key: &str,
    api_secret: &str,
    room: &str,
    now_seconds: u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    sign_livekit_claims(
        api_key,
        api_secret,
        "room-admin",
        now_seconds,
        json!({"room": room, "roomAdmin": true}),
    )
}

/// Validation-only: `roomList` proves the key works without needing a room,
/// unlike `livekit_room_admin_token`.
pub fn livekit_room_list_token(
    api_key: &str,
    api_secret: &str,
    now_seconds: u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    sign_livekit_claims(
        api_key,
        api_secret,
        "credential-check",
        now_seconds,
        json!({"roomList": true}),
    )
}

/// A credential for deleting a room, which LiveKit gates on `roomCreate`
/// rather than on `roomAdmin`.
///
/// No room, because LiveKit checks `roomCreate` project-wide and ignores any
/// room in the grant: naming one here would read as a scope nothing enforces.
/// Kept apart from [`livekit_room_admin_token`] so the cleanup path cannot
/// remove participants and the agent's path cannot delete rooms.
pub fn livekit_room_create_token(
    api_key: &str,
    api_secret: &str,
    now_seconds: u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    sign_livekit_claims(
        api_key,
        api_secret,
        "room-cleanup",
        now_seconds,
        json!({"roomCreate": true}),
    )
}

/// The claims every token here shares. The expiry is checked, not saturated:
/// a clock too close to `u64::MAX` must fail rather than mint a credential
/// that never expires.
fn livekit_claims(
    api_key: &str,
    identity: &str,
    now_seconds: u64,
    video: Value,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    livekit_claims_with_ttl(api_key, identity, now_seconds, TOKEN_TTL_SECONDS, video)
}

fn livekit_claims_with_ttl(
    api_key: &str,
    identity: &str,
    now_seconds: u64,
    ttl_seconds: u64,
    video: Value,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let exp = now_seconds
        .checked_add(ttl_seconds)
        .ok_or("token expiry overflows")?;
    Ok(json!({
        "iss": api_key,
        "sub": identity,
        "nbf": now_seconds,
        "exp": exp,
        "video": video
    }))
}

/// How long a management credential lives. Each is minted for one call, and a
/// leaked one can act project-wide, so it does not last as long as an
/// interview.
pub const MANAGEMENT_TOKEN_TTL_SECONDS: u64 = 300;

fn sign_livekit_claims(
    api_key: &str,
    api_secret: &str,
    identity: &str,
    now_seconds: u64,
    video: Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    sign_jwt(
        api_secret,
        &livekit_claims_with_ttl(
            api_key,
            identity,
            now_seconds,
            MANAGEMENT_TOKEN_TTL_SECONDS,
            video,
        )?,
    )
}

fn sign_jwt(
    api_secret: &str,
    claims: &Value,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let header = b64_json(&json!({ "alg": "HS256", "typ": "JWT" }))?;
    let signing_input = format!("{header}.{}", b64_json(claims)?);
    let signature = sign_hs256(api_secret, &signing_input);
    Ok(format!("{signing_input}.{signature}"))
}

fn b64_json(value: &Value) -> serde_json::Result<String> {
    serde_json::to_vec(value).map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
}

/// Why a LiveKit webhook was refused. One value per distinguishable cause,
/// because the handler answers all of them the same way and the operator
/// debugging a webhook that never arrives needs to know which one it was: a
/// clock skew and a stolen key are the same 401 and completely different
/// problems.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookRejection {
    /// Not three base64url segments carrying JSON claims.
    Malformed,
    /// Signed by a project this server does not serve.
    UnknownKey,
    /// The right key, the wrong secret.
    BadSignature,
    /// Outside the token's own validity window.
    Expired,
    /// A valid signature over bytes other than the ones that arrived. This is
    /// the case a signature alone does not catch, and the reason LiveKit puts
    /// a digest of the body inside the token.
    BodyMismatch,
}

impl std::fmt::Display for WebhookRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Malformed => "malformed",
            Self::UnknownKey => "unknown_key",
            Self::BadSignature => "bad_signature",
            Self::Expired => "expired",
            Self::BodyMismatch => "body_mismatch",
        })
    }
}

/// The API key a webhook says signed it, without checking whether it did.
///
/// `iss` selects the secret, so it has to be read before there is anything to
/// verify with. Split out from the verification so a multi-project deployment
/// can look the secret up: a room belongs to one LiveKit project, and the
/// primary project's secret verifies nothing that another one signed.
///
/// Nothing here is trusted. The value chooses which secret to try, and the
/// signature check is what decides.
pub fn livekit_token_issuer(authorization: &str) -> Option<String> {
    let (signing_input, _) = authorization.rsplit_once('.')?;
    let (_, payload) = signing_input.split_once('.')?;
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).ok()?).ok()?;
    claims
        .get("iss")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .map(str::to_string)
}

/// The claims of a token this server's own secret vouches for, or why it does
/// not.
///
/// Both inbound credentials go through here. They differ only in what they ask
/// of a token already known to be real: a webhook has to match a body digest, a
/// recorder's token has to name a room it may join. Everything before that is
/// the same check, and the module's argument against a second copy of the
/// signing step is the same argument against a second copy of this one. Two
/// prologues drift, and the half that drifts is the half that accepts something
/// the other refuses.
///
/// The order is forced. `iss` names the project whose secret verifies the
/// token, so it has to be read out of an unverified payload before there is
/// anything to verify with; every later step then runs against claims the
/// signature has already vouched for. Upstream's own receiver does the same.
///
/// The JWT header is never read, and that is the point. Algorithm confusion
/// needs a verifier that takes `alg` from the token; this one always computes
/// HMAC-SHA256, so `{"alg":"none"}` fails the signature check like any other
/// forgery. Parsing the header to reject what it already rejects would be code
/// defending against nothing. Raised in review twice; written down so it is not
/// raised a third time.
fn verified_claims(
    api_key: &str,
    api_secret: &str,
    token: &str,
    now_seconds: u64,
) -> Result<Value, WebhookRejection> {
    let (signing_input, signature) = token.rsplit_once('.').ok_or(WebhookRejection::Malformed)?;
    let (_, payload) = signing_input
        .split_once('.')
        .ok_or(WebhookRejection::Malformed)?;
    let claims: Value = URL_SAFE_NO_PAD
        .decode(payload)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or(WebhookRejection::Malformed)?;

    if claims.get("iss").and_then(Value::as_str) != Some(api_key) {
        return Err(WebhookRejection::UnknownKey);
    }
    if !verify_hs256(api_secret, signing_input, signature) {
        return Err(WebhookRejection::BadSignature);
    }

    // A token with no `exp` is refused rather than treated as eternal. LiveKit
    // always sets one, five minutes out, so the absent case is not a message
    // either caller has to keep working for.
    let expires_at = claims
        .get("exp")
        .and_then(Value::as_u64)
        .ok_or(WebhookRejection::Malformed)?;
    let not_before = claims.get("nbf").and_then(Value::as_u64).unwrap_or(0);
    if now_seconds >= expires_at || now_seconds < not_before {
        return Err(WebhookRejection::Expired);
    }
    Ok(claims)
}

/// Whether `authorization` is LiveKit's signature over exactly `body`.
///
/// The header is the bare JWT, with no `Bearer` prefix: that is what
/// `webhook/url_notifier.go` sets, and accepting a prefix it never sends would
/// only widen what this function has to be right about.
pub fn verify_livekit_webhook(
    api_key: &str,
    api_secret: &str,
    authorization: &str,
    body: &[u8],
    now_seconds: u64,
) -> Result<(), WebhookRejection> {
    let claims = verified_claims(api_key, api_secret, authorization, now_seconds)?;

    // Standard base64 with padding, matching `webhook/verifier.go`. A URL-safe
    // decoder here would reject every digest containing a `+` or a `/`, which
    // is most of them, and the failure would look like a forged message.
    //
    // Compared with `==` rather than in constant time. The digest is a public
    // function of a body the sender already holds, and the one secret-dependent
    // comparison happened in `verified_claims`, which uses
    // `ring::hmac::verify`. There is no secret here to leak the prefix length
    // of.
    let digest = STANDARD.encode(digest::digest(&digest::SHA256, body).as_ref());
    if claims.get("sha256").and_then(Value::as_str) != Some(digest.as_str()) {
        return Err(WebhookRejection::BodyMismatch);
    }
    Ok(())
}

/// The room a LiveKit join token was minted for, once the signature says the
/// token is real.
///
/// This is how the recording template proves who it is. It carries no session
/// cookie, only the token Egress minted for it, and that token is signed with
/// the project's own secret, which this server holds. Verifying it is the
/// difference between "the recorder of this room" and "anyone who guessed a
/// room name".
///
/// The room, not the identity: an Egress recorder joins hidden and its identity
/// is the provider's to choose, so the grant is what says which room this
/// credential is good for.
pub fn livekit_room_from_token(
    api_key: &str,
    api_secret: &str,
    token: &str,
    now_seconds: u64,
) -> Result<String, WebhookRejection> {
    let claims = verified_claims(api_key, api_secret, token, now_seconds)?;
    joined_room(&claims)
}

/// The room a LiveKit join token was minted for and the identity it joins as,
/// once the signature says the token is real.
///
/// This is how a page proves which start it is giving up. The room alone
/// does not say that: a fixed development room is handed out again and again,
/// and every one of those tokens names it. The identity is minted fresh for
/// each start, so it is what tells this start's token from an earlier one.
pub fn livekit_join_from_token(
    api_key: &str,
    api_secret: &str,
    token: &str,
    now_seconds: u64,
) -> Result<(String, String), WebhookRejection> {
    let claims = verified_claims(api_key, api_secret, token, now_seconds)?;
    let room = joined_room(&claims)?;
    let identity = claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|identity| !identity.is_empty())
        .ok_or(WebhookRejection::Malformed)?;
    Ok((room, identity.to_string()))
}

fn joined_room(claims: &Value) -> Result<String, WebhookRejection> {
    // `roomJoin` as well as the name. A token good for something other than
    // joining is neither a recorder's nor a candidate's, whatever room it
    // names.
    let grant = claims.get("video").ok_or(WebhookRejection::Malformed)?;
    if grant.get("roomJoin").and_then(Value::as_bool) != Some(true) {
        return Err(WebhookRejection::Malformed);
    }
    grant
        .get("room")
        .and_then(Value::as_str)
        .filter(|room| !room.is_empty())
        .map(str::to_string)
        .ok_or(WebhookRejection::Malformed)
}

/// A credential for starting and stopping one room's Egress.
///
/// Separate from [`livekit_room_admin_token`] because the grants are different
/// and neither should be able to do the other's job: `roomAdmin` can remove
/// participants and mutate the room, and `roomRecord` cannot, which is exactly
/// the point of handing this one to the recording path.
///
/// Room-scoped. A token that can record every room in the project is not a
/// credential a per-interview request needs.
pub fn livekit_egress_token(
    api_key: &str,
    api_secret: &str,
    room: &str,
    now_seconds: u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    sign_livekit_claims(
        api_key,
        api_secret,
        "recording",
        now_seconds,
        json!({"room": room, "roomRecord": true}),
    )
}

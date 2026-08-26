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
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// HMAC-SHA256 over `value`, base64url without padding.
///
/// The one place that knows the algorithm. The JWT signer below and the signed
/// cookies in `web` both go through it, because two copies of a signing step
/// are two chances for one of them to drift into something weaker.
pub fn sign_hs256(secret: &str, value: &str) -> String {
    // The only documented failure is a key length HMAC rejects, and HMAC
    // accepts every length: it hashes keys longer than the block size and pads
    // shorter ones.
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key");
    mac.update(value.as_bytes());
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

/// Whether `signature` is `sign_hs256(secret, value)`.
///
/// The comparison is `Mac::verify_slice`, which is constant time. A byte-wise
/// `==` on the base64 would leak how much of a forged signature was right.
pub fn verify_hs256(secret: &str, value: &str, signature: &str) -> bool {
    let Ok(signature) = URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key");
    mac.update(value.as_bytes());
    mac.verify_slice(&signature).is_ok()
}

pub const TOKEN_TTL_SECONDS: u64 = 2 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    let mut claims = json!({
        "iss": input.api_key,
        "sub": input.identity,
        "name": input.name,
        "nbf": input.now_seconds,
        "exp": input.now_seconds + TOKEN_TTL_SECONDS,
        "metadata": input.metadata,
        "video": {
            "room": input.room,
            "roomJoin": true,
            "canPublish": true,
            "canSubscribe": true,
            "canPublishData": true,
            "agent": input.agent,
            "canUpdateOwnMetadata": input.agent
        }
    });
    if input.agent {
        claims["kind"] = json!("agent");
    }
    sign_jwt(input.api_secret, &claims)
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
    sign_jwt(
        api_secret,
        &json!({
            "iss": api_key,
            "sub": identity,
            "name": "Interviewer observer",
            "nbf": now_seconds,
            "exp": now_seconds + TOKEN_TTL_SECONDS,
            "video": {
                "room": room,
                "roomJoin": true,
                "canPublish": false,
                "canSubscribe": true,
                "canPublishData": false,
                "canUpdateOwnMetadata": false
            }
        }),
    )
}

pub fn livekit_room_admin_token(
    api_key: &str,
    api_secret: &str,
    room: &str,
    now_seconds: u64,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    sign_jwt(
        api_secret,
        &json!({
            "iss": api_key,
            "sub": "room-admin",
            "nbf": now_seconds,
            "exp": now_seconds + TOKEN_TTL_SECONDS,
            "video": {
                "room": room,
                "roomAdmin": true
            }
        }),
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
    // comparison happened in `verified_claims`, which uses `Mac::verify_slice`.
    // There is no secret here to leak the prefix length of.
    let digest = STANDARD.encode(Sha256::digest(body));
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

    // `roomJoin` as well as the name. A token good for something other than
    // joining is not a recorder's, whatever room it names.
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
    sign_jwt(
        api_secret,
        &json!({
            "iss": api_key,
            "sub": "recording",
            "nbf": now_seconds,
            "exp": now_seconds + TOKEN_TTL_SECONDS,
            "video": {
                "room": room,
                "roomRecord": true
            }
        }),
    )
}

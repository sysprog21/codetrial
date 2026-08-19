//! LiveKit access tokens.
//!
//! Both the HTTP layer (candidate tokens) and the agent runtime (join and room
//! admin tokens) mint these, so the signing lives here rather than in either
//! caller.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

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

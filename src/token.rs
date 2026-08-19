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
    let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())?;
    mac.update(signing_input.as_bytes());
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    ))
}

fn b64_json(value: &Value) -> serde_json::Result<String> {
    serde_json::to_vec(value).map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
}

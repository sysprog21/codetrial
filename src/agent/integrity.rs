//! Integrity evidence: bounding what the browser reports, and hashing it into
//! a chain the server can check.
//!
//! Deliberately separate from the rest of the runtime state, because this is
//! the only place that treats browser input as hostile.
//!
//! # What the chain proves, and what it does not
//!
//! Every event carries `seq`, `prevHash` and `hash`. `apply_integrity` in the
//! parent module recomputes the hash, requires `seq` to be exactly one past the
//! last accepted event, and requires `prevHash` to match the last accepted
//! hash. So an event cannot be edited, reordered, dropped from the middle, or
//! replayed once the server has accepted its successor.
//!
//! That is the whole of it. The digest is unkeyed SHA-256 over fields the
//! browser supplies, and in a proctoring product the candidate owns the
//! browser. Nothing here stops someone computing a perfectly valid chain over
//! events that never happened, or over an interview with the inconvenient ones
//! left out. The chain establishes that the log was not altered after it was
//! submitted. It does not establish that the log is true, and no amount of
//! hashing on the reporting side can, because the reporter is the adversary.
//!
//! What can carry weight is what the server sees for itself. The agent already
//! observes LiveKit participant and track state directly: a camera that stops
//! publishing is visible server-side whether or not the browser says so. Treat
//! browser-authored events as a candidate's own account of the session, and
//! anything corroborated by the room as evidence.

use sha2::{Digest, Sha256};

use super::{MAX_INTEGRITY_TEXT, json_int};

pub fn sanitize_integrity_event(payload: &serde_json::Value) -> Option<serde_json::Value> {
    let text = |key| {
        payload
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(|value| value.chars().take(MAX_INTEGRITY_TEXT).collect::<String>())
    };
    let event_type = match text("type")?.as_str() {
        "SESSION_START" => "SESSION_START",
        "MEDIA_PREFLIGHT_PASSED" => "MEDIA_PREFLIGHT_PASSED",
        "SCREEN_SHARE_STARTED" => "SCREEN_SHARE_STARTED",
        "SCREEN_SHARE_STOPPED" => "SCREEN_SHARE_STOPPED",
        "SCREEN_SHARE_UNHEALTHY" => "SCREEN_SHARE_UNHEALTHY",
        "SCREEN_SHARE_RECOVERED" => "SCREEN_SHARE_RECOVERED",
        "SCREEN_CHANGED" => "SCREEN_CHANGED",
        "FACE_DETECTED" => "FACE_DETECTED",
        "FACE_MISSING" => "FACE_MISSING",
        "FACE_MISSING_SEVERE" => "FACE_MISSING_SEVERE",
        "FACE_DETECTOR_UNAVAILABLE" => "FACE_DETECTOR_UNAVAILABLE",
        "MULTIPLE_FACES" => "MULTIPLE_FACES",
        "REVIEW_EVENT" => "REVIEW_EVENT",
        "CAMERA_STOPPED" => "CAMERA_STOPPED",
        "MICROPHONE_STOPPED" => "MICROPHONE_STOPPED",
        "INTEGRITY_HEARTBEAT" => "INTEGRITY_HEARTBEAT",
        "SCREEN_MUTED" => "SCREEN_MUTED",
        "SCREEN_UNMUTED" => "SCREEN_UNMUTED",
        "CAMERA_MUTED" => "CAMERA_MUTED",
        "CAMERA_UNMUTED" => "CAMERA_UNMUTED",
        "MICROPHONE_MUTED" => "MICROPHONE_MUTED",
        "MICROPHONE_UNMUTED" => "MICROPHONE_UNMUTED",
        "SCREEN_STATE_CHANGED" => "SCREEN_STATE_CHANGED",
        "CAMERA_STATE_CHANGED" => "CAMERA_STATE_CHANGED",
        // Meet presentation mode hands the camera to Google Meet, so no camera
        // evidence exists for the rest of the session. Recorded as an event
        // rather than silence: a report with no face events must not be
        // mistaken for a report where the camera was watched and saw nothing.
        "CAMERA_RELEASED_TO_PRESENTER" => "CAMERA_RELEASED_TO_PRESENTER",
        "MICROPHONE_STATE_CHANGED" => "MICROPHONE_STATE_CHANGED",
        _ => return None,
    };
    let at = text("at")?;
    let severity = match text("severity")?.as_str() {
        "info" => "info",
        "warning" => "warning",
        "high" => "high",
        "critical" => "critical",
        _ => return None,
    };
    let source = match text("source")?.as_str() {
        "media" => "media",
        "preflight" => "preflight",
        "heartbeat" => "heartbeat",
        "screen" => "screen",
        "camera" => "camera",
        "microphone" => "microphone",
        "review" => "review",
        _ => return None,
    };
    let seq = payload.get("seq").and_then(serde_json::Value::as_u64)?;
    let prev_hash = text("prevHash")?;
    let hash = text("hash")?;
    if hash.len() != 64 || !hash.chars().all(|char| char.is_ascii_hexdigit()) {
        return None;
    }
    let duration_ms = payload
        .get("durationMs")
        .and_then(json_int)
        .unwrap_or(0)
        .clamp(0, 24 * 60 * 60 * 1000);
    let detail = text("detail").filter(|value| {
        value.chars().all(|char| {
            char.is_ascii_alphanumeric()
                || matches!(char, ' ' | '_' | '-' | '=' | '/' | ';' | ':' | ',' | '.')
        })
    });
    let source_event_ids = payload
        .get("sourceEventIds")
        .and_then(serde_json::Value::as_array)
        .map(|ids| {
            ids.iter()
                .filter_map(serde_json::Value::as_str)
                .filter(|value| value.chars().all(|char| char.is_ascii_digit()))
                .take(4)
                .map(|value| value.chars().take(12).collect::<String>())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(serde_json::json!({
        "type": event_type,
        "at": at,
        "severity": severity,
        "source": source,
        "durationMs": duration_ms,
        "seq": seq,
        "prevHash": prev_hash,
        "hash": hash,
        "detail": detail,
        "sourceEventIds": source_event_ids,
    }))
}

pub(crate) fn integrity_hash(event: &serde_json::Value) -> String {
    let mut body = serde_json::json!({
        "seq": event.get("seq").cloned().unwrap_or(serde_json::Value::Null),
        "prevHash": event.get("prevHash").cloned().unwrap_or(serde_json::Value::Null),
        "type": event.get("type").cloned().unwrap_or(serde_json::Value::Null),
        "at": event.get("at").cloned().unwrap_or(serde_json::Value::Null),
        "severity": event.get("severity").cloned().unwrap_or(serde_json::Value::Null),
        "source": event.get("source").cloned().unwrap_or(serde_json::Value::Null),
        "durationMs": event.get("durationMs").cloned().unwrap_or(serde_json::Value::Null),
        "detail": event.get("detail").cloned().unwrap_or(serde_json::Value::Null),
    });
    if event
        .get("sourceEventIds")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|ids| !ids.is_empty())
    {
        body["sourceEventIds"] = event
            .get("sourceEventIds")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
    }
    let digest = Sha256::digest(canonical_json(&body).as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        serde_json::Value::Object(object) => format!(
            "{{{}}}",
            object
                .iter()
                .map(|(key, value)| format!(
                    "{}:{}",
                    serde_json::to_string(key).unwrap_or_default(),
                    canonical_json(value)
                ))
                .collect::<Vec<_>>()
                .join(",")
        ),
        // Infallible for a `Value`: keys are always strings and a `Number`
        // cannot hold NaN. Written as a fallback anyway, because this runs on
        // candidate-supplied event data and a reviewer should not have to
        // reconstruct that argument to rule out a remote panic.
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

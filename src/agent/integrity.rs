//! Integrity evidence: bounding what the browser reports, and hashing it into
//! a chain the server can check.
//!
//! Deliberately separate from the rest of the runtime state, because this is
//! the only place that treats browser input as hostile. Test runs are bounded
//! here too, for the same reason and not because they are integrity evidence:
//! they arrive on a topic the candidate publishes, and every field of them is
//! read back to a model.
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

use super::{
    MAX_INTEGRITY_TEXT, MAX_TEST_CASES, MAX_TEST_FAILURES, MAX_TEST_TEXT, json_int, python_truthy,
    spoken_language,
};

/// Bounds a test-run packet before anything stores or renders it.
///
/// `apply_test_results` in the parent module says why these counts are narrated
/// and never scored. This is the other half of that boundary, and it is the
/// half that matters more: the counts are one number a reader can discount,
/// while `setupError` and the failure strings are free text that lands verbatim
/// in `read_editor_text` and in the report prompt. Unbounded, they are a way
/// for the candidate to write directly into the prompt that decides the hire.
///
/// Unlike `sanitize_integrity_event` this does not reject a run for being
/// malformed: one is still a run the interviewer should react to, so every
/// field falls back to something harmless rather than dropping the event. The
/// one thing it will not do is invent a run, which is what the empty case below
/// is about.
pub fn sanitize_test_run(payload: &serde_json::Value) -> serde_json::Value {
    // An absent or empty packet reads as "no run happened", and that has to
    // survive sanitizing. Bounding one into shape would build a packet out of
    // nothing and tell the report a run occurred and scored zero, which is the
    // same kind of false claim this function exists to stop.
    if !python_truthy(payload) {
        return serde_json::Value::Null;
    }

    // Line breaks go before the length bound, and they are the ones that
    // matter: `format_test_run` builds a line per failure and joins them, and
    // `test_results_reaction` wraps the result in a `[SYSTEM EVENT]` block. A
    // `got` carrying its own newline therefore writes a whole line of the
    // interviewer's prompt, and it can make that line look like ours.
    //
    // `is_control` is not the whole test. U+2028 and U+2029 are separators
    // rather than controls, so they pass it, and a model reading the prompt may
    // well break a line on them: the thing being defended is what the model
    // sees, not what `str::lines` splits on.
    //
    // Brackets stay: "expected [1, 2, 3], got [3, 2, 1]" is the string this
    // bound is generous enough to fit, and mangling it to protect against a
    // delimiter the model reads as prose costs more than it buys.
    let text = |value: Option<&serde_json::Value>| {
        value.and_then(serde_json::Value::as_str).map(|value| {
            value
                .chars()
                .filter(|c| !c.is_control() && !matches!(c, '\u{2028}' | '\u{2029}'))
                .take(MAX_TEST_TEXT)
                .collect::<String>()
        })
    };

    // Clamped against each other and not only against the ceiling.
    // `apply_test_results` reads `passed == total` as every case passing, so
    // two independent clamps map a claimed 100 of 150 onto 99 of 99 and hand
    // the congratulatory reaction to a run that failed a third of its cases. A
    // sanitizer is allowed to drop meaning, never to upgrade a failing run into
    // a clean one, so what carries across the ceiling is whether the claim was
    // "all of them" and not the digits it was written with.
    let claimed = |key| payload.get(key).and_then(json_int).unwrap_or(0).max(0);
    let claimed_total = claimed("total");
    let claimed_passed = claimed("passed").min(claimed_total);
    let total = claimed_total.min(MAX_TEST_CASES);
    let passed = if claimed_passed == claimed_total {
        total
    } else {
        // The branch is only reached with `claimed_passed < claimed_total`, so
        // `claimed_total` is at least one and `total - 1` cannot go negative.
        claimed_passed.min(total - 1)
    };
    let failures = payload
        .get("failures")
        .and_then(serde_json::Value::as_array)
        .map(|failures| {
            failures
                .iter()
                // Bounded before the filter, not after: filtering first walks
                // the whole array to decide it has nothing, so an array of
                // non-objects costs its full length to yield zero failures.
                .take(MAX_TEST_FAILURES)
                .filter_map(serde_json::Value::as_object)
                .map(|failure| {
                    serde_json::json!({
                        // "?" rather than null: the renderer prints `label`
                        // unconditionally, and a null reads as a test named
                        // None.
                        "label": text(failure.get("label")).unwrap_or_else(|| "?".to_string()),
                        "expected": text(failure.get("expected")),
                        "got": text(failure.get("got")),
                        "error": text(failure.get("error")),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    serde_json::json!({
        "passed": passed,
        "total": total,

        // The same allowlist the spoken acknowledgement uses, so a run cannot
        // claim a language the product does not offer. Note this stores the
        // display name, "Python" and not "python": the renderer speaks this
        // field, and the slug the rest of the runtime keys off lives in
        // `RuntimeState::language`.
        "language": payload
            .get("language")
            .and_then(serde_json::Value::as_str)
            .and_then(spoken_language)
            .unwrap_or("?"),
        "setupError": text(payload.get("setupError")),
        "failures": failures,
    })
}

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

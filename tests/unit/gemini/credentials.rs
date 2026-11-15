use super::*;
use crate::config::load_from_pairs;
use serde_json::json;

#[test]
fn quota_cooldown_ends_at_its_deadline() {
    let deadline = Instant::now();
    assert!(!cooling_down(None, deadline));
    assert!(cooling_down(
        Some(deadline),
        deadline - Duration::from_nanos(1)
    ));
    assert!(!cooling_down(Some(deadline), deadline));
    assert!(!cooling_down(
        Some(deadline),
        deadline + Duration::from_nanos(1)
    ));
}

#[test]
fn failure_messages_distinguish_transport_status_and_credentials() {
    for (status, expected) in [
        (0, "Gemini connection or setup failed"),
        (400, "Gemini request rejected (status=400)"),
        (503, "Gemini unavailable (status=503)"),
        (401, "Gemini credential rejected"),
        (429, "Gemini quota exhausted"),
    ] {
        assert_eq!(
            ApiFailure::from_response(status, &json!({})).to_string(),
            expected
        );
    }
}

/// The body's own reason survives a 5xx, on one line, because it is what
/// tells an overload from an outage.
#[test]
fn an_unavailable_gemini_keeps_the_reason_it_gave() {
    let body = json!({"error": {"code": 503, "status": "UNAVAILABLE",
        "message": "The model is overloaded.\nPlease try again later."}});
    assert_eq!(
        ApiFailure::from_response(503, &body).to_string(),
        "Gemini unavailable (status=503): The model is overloaded. Please try again later."
    );
    let long = json!({"error": {"message": "x".repeat(1_000)}});
    assert!(ApiFailure::from_response(500, &long).to_string().len() < 260);

    // Nothing usable in the body falls back to the status alone rather than
    // ending on a colon.
    for message in ["", "   ", "\n\t"] {
        let body = json!({"error": {"message": message}});
        assert_eq!(
            ApiFailure::from_response(503, &body).to_string(),
            "Gemini unavailable (status=503)"
        );
    }

    // Below 500 a message is not an outage's reason, so it is not quoted as
    // one.
    let body = json!({"error": {"message": "Invalid JSON payload received."}});
    assert_eq!(
        ApiFailure::from_response(400, &body).to_string(),
        "Gemini request rejected (status=400)"
    );
}

#[test]
fn first_report_selection_starts_from_the_working_live_key() {
    let keys = GeminiKeys::new(vec!["live-seed-a".into(), "live-seed-b".into()]);
    keys.failed("live-seed-a", CredentialFailure::Quota, ApiSurface::Live);
    assert_eq!(keys.select().unwrap(), "live-seed-b");
    assert_eq!(keys.select_report().unwrap(), "live-seed-b");
    // A remains available to reports; B was selected because Live uses it.
    let other = GeminiKeys::new(vec!["live-seed-a".into(), "live-seed-b".into()]);
    assert_eq!(other.select_report().unwrap(), "live-seed-a");
}

#[test]
fn quota_is_shared_per_surface_but_invalid_keys_are_shared_globally() {
    let pair = |prefix: &str| GeminiKeys::new(vec![format!("{prefix}-a"), format!("{prefix}-b")]);
    let first = pair("surface-isolation");
    let second = pair("surface-isolation");
    first.failed(
        "surface-isolation-a",
        CredentialFailure::Quota,
        ApiSurface::Live,
    );
    assert_eq!(second.select().unwrap(), "surface-isolation-b");
    assert_eq!(
        pair("surface-isolation").select_report().unwrap(),
        "surface-isolation-a"
    );
    first.failed(
        "surface-isolation-b",
        CredentialFailure::Invalid,
        ApiSurface::Live,
    );
    first.failed(
        "surface-isolation-a",
        CredentialFailure::Quota,
        ApiSurface::Report,
    );
    assert!(pair("surface-isolation").select().is_err());
    assert!(pair("surface-isolation").select_report().is_err());

    let first = pair("report-surface-isolation");
    first.failed(
        "report-surface-isolation-a",
        CredentialFailure::Quota,
        ApiSurface::Report,
    );
    assert_eq!(
        pair("report-surface-isolation").select_report().unwrap(),
        "report-surface-isolation-b"
    );
    assert_eq!(
        pair("report-surface-isolation").select().unwrap(),
        "report-surface-isolation-a"
    );
    first.failed(
        "report-surface-isolation-a",
        CredentialFailure::Invalid,
        ApiSurface::Report,
    );
    assert_eq!(
        pair("report-surface-isolation").select().unwrap(),
        "report-surface-isolation-b"
    );
}

/// With nothing to move to, marking the only key would refuse every room and
/// every restart that followed. That holds when another interview's list
/// carries the same key string and marks it there.
#[test]
fn a_sole_key_is_never_taken_out_of_rotation() {
    let sole = GeminiKeys::single("sole-rotation-a");
    for failure in [CredentialFailure::Quota, CredentialFailure::Invalid] {
        for surface in [ApiSurface::Live, ApiSurface::Report] {
            sole.failed("sole-rotation-a", failure, surface);
        }
    }
    let fresh = GeminiKeys::new(vec!["sole-rotation-a".into(), "sole-rotation-b".into()]);
    assert_eq!(fresh.select().unwrap(), "sole-rotation-a");
    assert_eq!(fresh.select_report().unwrap(), "sole-rotation-a");
    let list = GeminiKeys::new(vec!["sole-rotation-a".into(), "sole-rotation-b".into()]);
    list.failed(
        "sole-rotation-a",
        CredentialFailure::Invalid,
        ApiSurface::Live,
    );
    assert_eq!(list.select().unwrap(), "sole-rotation-b");
    assert_eq!(sole.select().unwrap(), "sole-rotation-a");
    assert_eq!(sole.select_report().unwrap(), "sole-rotation-a");
}

/// Expires a cooldown without waiting, after checking it was armed: an insert
/// of an already-past deadline would pass whether the failure took or not.
fn expire_live_cooldown(key: &str) {
    let mut cooldowns = COOLDOWNS.get().unwrap().lock().unwrap();
    assert!(cooling_down(cooldowns[key].live, Instant::now()), "{key}");
    cooldowns.remove(key);
}

#[test]
fn interviews_share_failures_but_keep_their_working_key() {
    let keys = vec!["shared-first".to_string(), "shared-second".to_string()];
    let first = GeminiKeys::new(keys.clone());
    let second = GeminiKeys::new(keys);
    assert_eq!(first.select().unwrap(), "shared-first");
    first.failed("shared-first", CredentialFailure::Quota, ApiSurface::Live);
    assert_eq!(second.select().unwrap(), "shared-second");
    // Once the cooldown is over, the working key must stay selected.
    expire_live_cooldown("shared-first");
    assert_eq!(first.select().unwrap(), "shared-first");
    assert_eq!(second.select().unwrap(), "shared-second");
    second.failed(
        "shared-second",
        CredentialFailure::Invalid,
        ApiSurface::Live,
    );
    second.failed("shared-second", CredentialFailure::Quota, ApiSurface::Live);
    assert_eq!(second.select().unwrap(), "shared-first");
    second.failed("shared-first", CredentialFailure::Invalid, ApiSurface::Live);
    let error = first.select().unwrap_err().to_string();
    assert!(error.contains("credentials exhausted"));
    assert!(!error.contains("shared-first"));
    assert!(GeminiKeys::single("").select().is_err());
}

#[test]
fn only_credential_or_quota_errors_disable_a_key() {
    for (status, body, expected) in [
        (429, json!({}), Some(CredentialFailure::Quota)),
        (401, json!({}), Some(CredentialFailure::Invalid)),
        (
            403,
            json!({"error":{"details":[{"reason":"QUOTA_EXCEEDED"}]}}),
            Some(CredentialFailure::Quota),
        ),
        (
            403,
            json!({"error":{"details":[{"reason":"RATE_LIMIT_EXCEEDED"}]}}),
            Some(CredentialFailure::Quota),
        ),
        (
            400,
            json!({"error":{"status":"RESOURCE_EXHAUSTED"}}),
            Some(CredentialFailure::Quota),
        ),
        (
            403,
            json!({"error":{"status":"RESOURCE_EXHAUSTED"}}),
            Some(CredentialFailure::Quota),
        ),
        (503, json!({"error":{"status":"RESOURCE_EXHAUSTED"}}), None),
        // Nothing explains these, and a 403 refuses the key either way, on the
        // surface that saw it. A handshake rejection carries no body.
        (
            403,
            json!({"error":{"details":[{"reason":"IAM_PERMISSION_DENIED"}]}}),
            Some(CredentialFailure::Refused),
        ),
        (403, Value::Null, Some(CredentialFailure::Refused)),
        (400, Value::Null, None),
        (
            400,
            json!({"error":{"details":[{"reason":"API_KEY_INVALID"}]}}),
            Some(CredentialFailure::Invalid),
        ),
        (
            403,
            json!({"error":{"message":"Your API key was reported as leaked. Please use another API key."}}),
            Some(CredentialFailure::Invalid),
        ),
        (400, json!({"error":{"message":"unsupported model"}}), None),
        (
            403,
            json!({"error":{"message":"model access denied"}}),
            Some(CredentialFailure::Refused),
        ),
        (404, json!({}), None),
        (500, json!({}), None),
        (503, json!({}), None),
    ] {
        let failure = ApiFailure::from_response(status, &body);
        assert_eq!(failure.credential, expected, "status={status}");
        assert!(!failure.to_string().contains("API_KEY_INVALID"));
    }
    for reason in [
        "API_KEY_INVALID",
        "API_KEY_EXPIRED",
        "API_KEY_SERVICE_BLOCKED",
        "API_KEY_IP_ADDRESS_BLOCKED",
        "API_KEY_HTTP_REFERRER_BLOCKED",
        "API_KEY_ANDROID_APP_BLOCKED",
        "API_KEY_IOS_APP_BLOCKED",
    ] {
        // A 400 names the key only through its reason.
        assert_eq!(
            ApiFailure::from_response(400, &json!({"error":{"details":[{"reason":reason}]}}))
                .credential,
            Some(CredentialFailure::Invalid),
            "{reason}"
        );
    }

    // A Live close frame carries the same codes as a response body, inside a
    // sentence and in any case.
    for reason in [
        "RESOURCE_EXHAUSTED",
        "Quota exceeded",
        "Quota exhausted",
        "QUOTA_EXCEEDED",
        "RATE_LIMIT_EXCEEDED",
        "reason: rate_limit_exceeded",
    ] {
        assert_eq!(
            failure_from_reason(reason),
            Some(CredentialFailure::Quota),
            "{reason}"
        );
    }
    for reason in [
        "API key not valid",
        "API_KEY_INVALID",
        "API key was reported as leaked",
        "API key has expired",
        "API key is disabled",
        "API_KEY_EXPIRED",
        "API_KEY_SERVICE_BLOCKED",
        "API_KEY_IP_ADDRESS_BLOCKED",
        "API_KEY_HTTP_REFERRER_BLOCKED",
        "API_KEY_ANDROID_APP_BLOCKED",
        "API_KEY_IOS_APP_BLOCKED",
        "Request refused: api_key_expired",
    ] {
        assert_eq!(
            failure_from_reason(reason),
            Some(CredentialFailure::Invalid),
            "{reason}"
        );
    }
    assert_eq!(failure_from_reason("unsupported model"), None);
    assert_eq!(failure_from_reason("service unavailable"), None);
}

/// The close reason a depleted prepaid account produced in the field, and the
/// status the HTTP calls got for it in the same session. Billing, not quota:
/// waiting out a quota cooldown and trying the same project again spends the
/// restart budget on a result that cannot change.
#[test]
fn a_project_that_cannot_pay_is_a_billing_failure() {
    for reason in [
        "Your prepayment credits are depleted. Please go to AI Studio to manage your project and billing.",
        "RESOURCE_EXHAUSTED: prepayment credits are depleted",
        "Billing account is not active",
        "Billing is not enabled for this project.",
        "billing has not been enabled on the project",
        "BILLING_DISABLED",
    ] {
        assert_eq!(
            failure_from_reason(reason),
            Some(CredentialFailure::Billing),
            "{reason}"
        );
    }

    // The rate-limit text mentions billing too, and it is a rate limit.
    let rate_limit = "You exceeded your current quota, please check your plan and billing details. For more information on this error, head to: https://ai.google.dev/gemini-api/docs/rate-limits.";
    assert_eq!(
        failure_from_reason(rate_limit),
        Some(CredentialFailure::Quota)
    );
    assert_eq!(
        ApiFailure::from_response(400, &json!({"error":{"message": rate_limit}})).credential,
        Some(CredentialFailure::Quota)
    );
    let failure = ApiFailure::from_response(402, &json!({}));
    assert_eq!(failure.credential, Some(CredentialFailure::Billing));
    assert_eq!(
        failure.to_string(),
        "Gemini billing or prepaid credit exhausted"
    );
    assert_eq!(
        ApiFailure::from_response(
            403,
            &json!({"error":{"message":"This API method requires billing to be enabled."}})
        )
        .credential,
        Some(CredentialFailure::Billing)
    );

    // Worded as an exhausted resource, and still not something waiting fixes.
    let depleted = json!({"error":{
        "status": "RESOURCE_EXHAUSTED",
        "message": "Your prepayment credits are depleted. Please go to AI Studio to manage your project and billing.",
    }});
    for status in [400, 403, 429] {
        assert_eq!(
            ApiFailure::from_response(status, &depleted).credential,
            Some(CredentialFailure::Billing),
            "{status}"
        );
    }
    assert_eq!(
        ApiFailure::from_response(429, &json!({"error":{"message": rate_limit}})).credential,
        Some(CredentialFailure::Quota)
    );
}

#[test]
fn a_billing_failure_takes_the_key_off_both_surfaces() {
    let keys = GeminiKeys::new(vec!["billing-a".into(), "billing-b".into()]);
    assert_eq!(keys.select().unwrap(), "billing-a");
    keys.failed("billing-a", CredentialFailure::Billing, ApiSurface::Live);
    assert_eq!(keys.select().unwrap(), "billing-b");
    assert_eq!(keys.select_report().unwrap(), "billing-b");
    keys.failed("billing-b", CredentialFailure::Billing, ApiSurface::Report);

    // Out for good as far as this rotation can tell, so there is no quota
    // cooldown worth waiting for.
    let error = keys.select().unwrap_err();
    assert_eq!(crate::gemini::exhausted_until(&error), None);
}

#[test]
fn failover_visits_the_next_key_before_returning_to_a_recovered_one() {
    let keys = GeminiKeys::new(vec![
        "ordered-a".into(),
        "ordered-b".into(),
        "ordered-c".into(),
    ]);
    keys.failed(
        &keys.select().unwrap(),
        CredentialFailure::Quota,
        ApiSurface::Live,
    );
    assert_eq!(keys.select().unwrap(), "ordered-b");
    // Recover the earlier key before failing the current one.
    expire_live_cooldown("ordered-a");
    keys.failed("ordered-b", CredentialFailure::Invalid, ApiSurface::Live);
    assert_eq!(keys.select().unwrap(), "ordered-c");
    keys.failed("ordered-c", CredentialFailure::Invalid, ApiSurface::Live);
    assert_eq!(keys.select().unwrap(), "ordered-a");
}

#[test]
fn redaction_covers_every_candidate_and_encoded_keys() {
    let keys = GeminiKeys::new(vec!["secret/one".into(), "secret+two".into()]);
    assert_eq!(
        keys.redact("secret/one secret%2Fone secret+two secret%2Btwo"),
        "[REDACTED] [REDACTED] [REDACTED] [REDACTED]"
    );
}

#[test]
fn ordered_google_keys_validate_without_requiring_a_legacy_key() {
    let base = [
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
    ];
    let config = load_from_pairs(
        base.into_iter()
            .chain([("GOOGLE_API_KEYS", " first, second ")]),
    )
    .unwrap();
    assert_eq!(config.google_api_keys, ["first", "second"]);
    for value in ["first,", ",second", "first, first"] {
        let error = load_from_pairs(base.into_iter().chain([("GOOGLE_API_KEYS", value)]))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("non-empty, distinct"));
        assert!(!error.contains("first"));
    }
    assert!(load_from_pairs(base).is_err());
}

/// A blank list is how an unset variable usually arrives, so it is no list:
/// it neither outranks the legacy key nor stands in for a missing one.
#[test]
fn a_blank_key_list_is_no_list() {
    let base = [
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
    ];
    for blank in ["", "   "] {
        let config = load_from_pairs(
            base.into_iter()
                .chain([("GOOGLE_API_KEYS", blank), ("GOOGLE_API_KEY", "legacy")]),
        )
        .unwrap();
        assert_eq!(config.google_api_keys, ["legacy"], "{blank:?}");
        assert_eq!(GeminiKeys::from_config(&config).select().unwrap(), "legacy");

        let error = load_from_pairs(base.into_iter().chain([("GOOGLE_API_KEYS", blank)]))
            .err()
            .unwrap();
        assert_eq!(error.missing_keys, ["GOOGLE_API_KEY"], "{blank:?}");
        assert!(error.invalid_entries.is_empty(), "{blank:?}");
    }
}

/// A project-wide refusal marks every key, so the rotation has to come back on
/// its own rather than stay empty for the life of the process.
#[test]
fn a_rejected_key_returns_after_its_cooldown() {
    let keys = GeminiKeys::new(vec!["rejected-return-a".into(), "rejected-return-b".into()]);
    keys.failed(
        "rejected-return-a",
        CredentialFailure::Invalid,
        ApiSurface::Live,
    );
    keys.failed(
        "rejected-return-b",
        CredentialFailure::Invalid,
        ApiSurface::Live,
    );
    let armed = {
        let cooldowns = COOLDOWNS.get().unwrap().lock().unwrap();
        let now = Instant::now();
        ["rejected-return-a", "rejected-return-b"].map(|key| {
            cooling_down(cooldowns[key].live, now) && cooling_down(cooldowns[key].report, now)
        })
    };
    assert_eq!(armed, [true, true]);
    assert!(keys.select().is_err());
    assert!(keys.select_report().is_err());
    expire_live_cooldown("rejected-return-b");
    assert_eq!(keys.select().unwrap(), "rejected-return-b");
    assert_eq!(keys.select_report().unwrap(), "rejected-return-b");
}

/// A bare 403 only the report model sees, such as a report model the project
/// cannot use, walked every key and parked all of them on Live as well.
#[test]
fn an_unexplained_refusal_stays_on_its_own_surface() {
    let keys = GeminiKeys::new(vec!["report-refused-a".into(), "report-refused-b".into()]);
    for key in ["report-refused-a", "report-refused-b"] {
        keys.failed(key, CredentialFailure::Refused, ApiSurface::Report);
    }
    assert!(keys.select_report().is_err());
    assert_eq!(keys.select().unwrap(), "report-refused-a");
}

/// Live waits for a quota key; Report exposes that deadline for the candidate
/// to request recovery. A rejection on one surface must not poison the other.
#[test]
fn an_exhausted_rotation_names_its_first_quota_key_back() {
    let keys = GeminiKeys::new(vec!["exhausted-wait-a".into(), "exhausted-wait-b".into()]);
    keys.failed(
        "exhausted-wait-a",
        CredentialFailure::Invalid,
        ApiSurface::Report,
    );
    keys.failed(
        "exhausted-wait-b",
        CredentialFailure::Quota,
        ApiSurface::Live,
    );
    let quota_back = COOLDOWNS.get().unwrap().lock().unwrap()["exhausted-wait-b"].live;
    assert_eq!(exhausted_until(&keys.select().unwrap_err()), quota_back);
    keys.failed(
        "exhausted-wait-b",
        CredentialFailure::Quota,
        ApiSurface::Report,
    );
    let report_back = COOLDOWNS.get().unwrap().lock().unwrap()["exhausted-wait-b"].report;
    assert_eq!(
        exhausted_until(&keys.select_report().unwrap_err()),
        report_back
    );

    keys.failed(
        "exhausted-wait-b",
        CredentialFailure::Refused,
        ApiSurface::Live,
    );
    assert_eq!(exhausted_until(&keys.select().unwrap_err()), None);
    assert_eq!(
        exhausted_until(&keys.select_report().unwrap_err()),
        report_back
    );
    keys.failed(
        "exhausted-wait-b",
        CredentialFailure::Refused,
        ApiSurface::Report,
    );
    assert_eq!(exhausted_until(&keys.select_report().unwrap_err()), None);
}

/// Slow quota rejections from enough keys outlast the first key's cooldown.
/// The server stands in for that by expiring every cooldown before it
/// answers, so a check bounded only by selection would never stop.
#[tokio::test]
#[expect(
    clippy::result_large_err,
    reason = "the handshake callback's error type is a full HTTP response"
)]
async fn check_tries_each_key_once_even_when_cooldowns_expire() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio_tungstenite::tungstenite::handshake::server;

    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        (
            "GOOGLE_API_KEYS",
            "check-bounded-a,check-bounded-b,check-bounded-c",
        ),
        ("GEMINI_LIVE_MODEL", "gemini-live"),
    ])
    .unwrap();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let connections = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&connections);
    let server = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            seen.fetch_add(1, Ordering::SeqCst);
            COOLDOWNS
                .get_or_init(Mutex::default)
                .lock()
                .unwrap()
                .retain(|key, _| !key.starts_with("check-bounded"));
            let _ = tokio_tungstenite::accept_hdr_async(
                socket,
                |_: &server::Request, _: server::Response| {
                    let mut refusal = server::ErrorResponse::new(None);
                    *refusal.status_mut() = axum::http::StatusCode::TOO_MANY_REQUESTS;
                    Err(refusal)
                },
            )
            .await;
        }
    });
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        super::super::startup::check_live_session_at(&url, &keys, &boot),
    )
    .await
    .expect("the check must stop after one attempt per key");
    server.abort();
    assert!(result.is_err());
    assert_eq!(connections.load(Ordering::SeqCst), 3);
}

/// Only a rotation that ran dry with no cooldown and no billing cause stands
/// for the refusals before it; a timed cooldown and a billing exhaustion name
/// their own cause, and a provider error is not an empty rotation at all.
#[test]
fn only_a_causeless_empty_rotation_ran_dry() {
    assert!(rotation_ran_dry(&exhausted(None, false)));
    assert!(!rotation_ran_dry(&exhausted(Some(Instant::now()), false)));
    assert!(!rotation_ran_dry(&exhausted(None, true)));
    assert!(!rotation_ran_dry(&exhausted(Some(Instant::now()), true)));
    assert!(!rotation_ran_dry(&io::Error::other("1011 Internal error")));
}

//! The interview record itself: creating one, ending it, the consent that
//! gates recording, the replay events it accumulates, and the reports written
//! against it.

use std::time::Instant;

use axum::body::{Body, to_bytes};
use axum::extract::{Path as UriPath, State};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::accounts::{
    MAX_INTERVIEWS_PER_USER, MAX_REPORTS_PER_USER, ReportSave, blocking, create_interview,
    delete_reports, list_reports, random_token, save_report,
};
use crate::current_epoch_seconds;

use super::auth::Owner;
use super::recordings::finish_recording;
use super::{
    AppState, MAX_BODY_BYTES, MAX_REPORT_BYTES, json_response, rate_limited_response,
    replay_response,
};

/// What one account posting replay events may spend in a window.
///
/// Derived from the only first-party caller rather than picked.
/// `web/replay-feed.js` flushes on a `REPLAY_FLUSH_MS = 1000` timer and again
/// whenever its queue fills, so an honest browser posts about sixty times a
/// minute with short bursts over that. Twice that leaves the bursts room and
/// still cuts a looping browser from thousands a second to two.
pub const REPLAY_RATE_LIMIT: u32 = 120;

pub(crate) async fn list_reports_handler(Owner { accounts, user }: Owner) -> Response {
    match blocking(move || list_reports(&accounts, user.id)).await {
        Ok(reports) => json_response(StatusCode::OK, json!({ "reports": reports })),
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not read reports." }),
        ),
    }
}

pub(crate) async fn delete_reports_handler(Owner { accounts, user }: Owner) -> Response {
    match blocking(move || delete_reports(&accounts, user.id)).await {
        Ok(deleted) => json_response(StatusCode::OK, json!({ "deleted": deleted })),
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not delete reports." }),
        ),
    }
}

pub(crate) async fn save_report_handler(
    Owner { accounts, user }: Owner,
    request: Request<Body>,
) -> Response {
    let Ok(body) = to_bytes(request.into_body(), MAX_REPORT_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Report must be JSON." }),
        );
    };
    let Some(problem_id) = payload
        .get("problemId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Report is missing problemId." }),
        );
    };
    let report_id = payload
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| random_token(18).ok());
    let Some(report_id) = report_id else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not create report id." }),
        );
    };
    let problem_id = problem_id.to_string();
    let saved = {
        let report_id = report_id.clone();
        blocking(move || save_report(&accounts, user.id, &report_id, &problem_id, &payload)).await
    };
    match saved {
        Ok(ReportSave::Saved) => json_response(StatusCode::OK, json!({ "id": report_id })),
        Ok(ReportSave::NotOwner) => json_response(
            StatusCode::CONFLICT,
            json!({ "error": "Report id belongs to another user." }),
        ),

        // Not 429: waiting does not help, and telling a client to retry a
        // request that can never succeed is worse than saying it is full.
        Ok(ReportSave::AtCapacity) => json_response(
            StatusCode::INSUFFICIENT_STORAGE,
            json!({ "error": format!("This account is at its limit of {MAX_REPORTS_PER_USER} saved reports.") }),
        ),
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not save report." }),
        ),
    }
}

/// Records consent, before anything can be recorded.
///
/// This is the ordering the whole feature rests on: the row exists first, and
/// `/api/token` refuses without it, so there is no path where an Egress call
/// precedes a candidate agreeing to one. The refusal is the enforcement; a
/// comment saying "call this first" is not.
pub(crate) async fn create_interview_handler(
    State(state): State<AppState>,
    Owner { accounts, user }: Owner,
    request: Request<Body>,
) -> Response {
    // A server that records nothing has no consent to take. Refusing keeps a
    // stale page from writing rows that mean nothing.
    if state.config.recording.is_none() {
        return json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_enabled", "error": "This server does not record interviews." }),
        );
    }
    let Ok(body) = to_bytes(request.into_body(), MAX_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let version = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|body| body.get("consentVersion")?.as_str().map(str::to_string));

    // The version has to match, not merely be present. It names the wording the
    // candidate was actually shown, and a page left open across a deploy that
    // changed that wording would otherwise be recorded as having agreed to text
    // it never displayed.
    if version.as_deref() != Some(crate::recording::CONSENT_VERSION) {
        return json_response(
            StatusCode::CONFLICT,
            json!({
                "code": "consent_version_mismatch",
                "error": "The recording disclosure changed. Reload the page and read it again."
            }),
        );
    }

    let Ok(interview_id) = random_token(16) else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not start an interview." }),
        );
    };
    let now = current_epoch_seconds() as i64;
    let recorded = {
        let interview_id = interview_id.clone();
        blocking(move || {
            create_interview(
                &accounts,
                &interview_id,
                user.id,
                crate::recording::CONSENT_VERSION,
                now,
            )
        })
        .await
    };
    match recorded {
        Ok(Some(interview)) => json_response(
            StatusCode::CREATED,
            json!({
                "interviewId": interview.id,
                "consentVersion": interview.consent_version,
                "consentAt": interview.consent_at
            }),
        ),

        // Not 429, for the same reason `/api/reports` is not: waiting does not
        // help, and telling a client to retry a request that can never succeed
        // is worse than saying it is full.
        Ok(None) => json_response(
            StatusCode::INSUFFICIENT_STORAGE,
            json!({
                "code": "interview_limit_reached",
                "error": format!("This account is at its limit of {MAX_INTERVIEWS_PER_USER} interviews.")
            }),
        ),
        Err(error) => {
            eprintln!("could not record interview consent: {error}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not record consent." }),
            )
        }
    }
}

/// The interview is over.
///
/// One of the two ways a recording stops normally, and neither is optional: the
/// browser can be closed and the `room_finished` webhook can be lost, so the
/// pipeline has to survive either arriving alone and both arriving in any
/// order.
pub(crate) async fn end_interview_handler(
    State(state): State<AppState>,
    UriPath(interview_id): UriPath<String>,
    Owner { accounts, user }: Owner,
) -> Response {
    let Some(recorder) = state.recorder.clone() else {
        return StatusCode::NO_CONTENT.into_response();
    };
    let found = {
        let accounts = accounts.clone();
        let interview_id = interview_id.clone();
        blocking(move || {
            crate::recording::recording_for_interview(&accounts, &interview_id, user.id)
        })
        .await
    };
    match found {
        // Nothing recorded, so there is nothing to stop. Not a 404: the
        // interview ending is the candidate's news either way.
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Ok(Some(recording)) => {
            finish_recording(&state, &accounts, &recorder, &recording, None).await
        }
        Err(error) => {
            eprintln!("could not read a recording while ending an interview: {error}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not end the interview." }),
            )
        }
    }
}

/// The replay, arriving one batch at a time.
///
/// Authenticated and owner-scoped like everything else about an interview, and
/// redacted before it is stored: this is the last place a payload is the
/// candidate's and the first it is this server's.
pub(crate) async fn replay_events_handler(
    State(state): State<AppState>,
    UriPath(interview_id): UriPath<String>,
    Owner { accounts, user }: Owner,
    request: Request<Body>,
) -> Response {
    use crate::recording::{Ingest, MAX_REPLAY_BATCH, MAX_REPLAY_BATCH_BYTES};

    if state.recorder.is_none() {
        return json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_enabled", "error": "This server does not record interviews." }),
        );
    }

    // Before the body, so a refused batch costs less than an accepted one: the
    // per-batch caps bound one request and the stored quota bounds what an
    // interview keeps, but neither bounds how often a looping browser may ask.
    // After the recorder check, which is free and answers the same for every
    // request.
    //
    // Keyed on the account, which `Owner` has already resolved. The address
    // would put a whole office behind one budget, where a single looping
    // browser could refuse everyone beside it.
    if !state.replay_limit.allow(user.id, Instant::now()) {
        return rate_limited_response("Too much replay at once. Wait a minute.");
    }

    let Ok(body) = to_bytes(request.into_body(), MAX_REPLAY_BATCH_BYTES).await else {
        return json_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({ "code": "replay_batch_too_large", "error": "Too much replay at once." }),
        );
    };
    let Some(events) = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|body| body.get("events")?.as_array().cloned())
    else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "code": "replay_envelope_invalid", "error": "Replay events must be a list." }),
        );
    };
    if events.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "code": "replay_envelope_invalid", "error": "A batch with no events in it." }),
        );
    }
    if events.len() > MAX_REPLAY_BATCH {
        return json_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({ "code": "replay_batch_too_large", "error": "Too many events at once." }),
        );
    }

    // Every event, or none. A batch that stored its first half and refused the
    // second would be a replay with a hole in it, and the producer has no way
    // to know which half survived.
    let mut parsed = Vec::with_capacity(events.len());
    for event in &events {
        match crate::recording::parse_replay_event(event) {
            Ok(event) => parsed.push(event),
            Err(rejection) => {
                return json_response(
                    if rejection == crate::recording::ReplayRejection::Oversize {
                        StatusCode::PAYLOAD_TOO_LARGE
                    } else {
                        StatusCode::BAD_REQUEST
                    },
                    json!({ "code": rejection.as_str(), "error": "That is not a replay event." }),
                );
            }
        }
    }

    let now = current_epoch_seconds() as i64;
    let stored = blocking(move || {
        crate::recording::append_replay_events(&accounts, &interview_id, user.id, &parsed, now)
    })
    .await;
    match stored {
        Ok(Ingest::Stored { first, last }) => json_response(
            StatusCode::OK,
            json!({ "firstSeq": first, "lastSeq": last }),
        ),

        // Not a failed recording. A replay that stopped growing is still a
        // recording worth keeping, and the flag is where a reviewer will see
        // that its tail is missing.
        Ok(Ingest::QuotaExceeded) => json_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            json!({
                "code": "replay_quota_exceeded",
                "error": "This interview has recorded as much replay as it can."
            }),
        ),
        Ok(Ingest::Empty) => json_response(
            StatusCode::BAD_REQUEST,
            json!({ "code": "replay_envelope_invalid", "error": "A batch with no events in it." }),
        ),
        Ok(Ingest::NoInterview) => json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_found", "error": "No such interview." }),
        ),
        Err(error) => {
            eprintln!("could not store replay events: {error}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not store the replay." }),
            )
        }
    }
}

/// Where a late join starts.
///
/// The snapshot is the replay with the superseded frames dropped, and `seq` is
/// where the caller carries on from. Nothing is stored to produce it and there
/// is no cadence to configure.
pub(crate) async fn replay_snapshot_handler(
    State(state): State<AppState>,
    UriPath(interview_id): UriPath<String>,
    Owner { accounts, user }: Owner,
) -> Response {
    let missing = || {
        json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_found", "error": "No replay for that interview." }),
        )
    };
    if state.recorder.is_none() {
        return missing();
    }
    let now = current_epoch_seconds() as i64;
    let view =
        blocking(move || crate::recording::replay_snapshot(&accounts, &interview_id, user.id, now))
            .await;

    // Gone rather than not-found on an expired or deleted replay: this account
    // owns the interview and is owed the difference between "never yours" and
    // "not any more". That distinction, and the two sentences carrying it, are
    // `replay_response`'s, shared with the two recording routes.
    replay_response(view, missing, "could not read a replay snapshot")
}

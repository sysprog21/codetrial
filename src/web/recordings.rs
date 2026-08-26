//! Everything downstream of a room being recorded: starting and stopping an
//! Egress job, the webhook that reports on it, the queries that list and serve
//! the result, and the background workers that sweep and deliver.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{Path as UriPath, Query, State};
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::accounts::{Accounts, blocking, random_token};
use crate::current_epoch_seconds;

use super::auth::signed_in_owner;
use super::consent::recording_requires_consent;
use super::{AppState, MAX_WEBHOOK_BODY_BYTES, json_response, replay_body};

/// How often the sweeper runs. The shortest retry backoff, because a schedule
/// checked less often than its own first step is a schedule with a different
/// first step.
pub(crate) const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// Resolves recordings nobody is looking after, now and then repeatedly.
///
/// Now, because a process that died mid-interview left rows no request will
/// ever touch again. Repeatedly, because the retry schedule is minutes long and
/// a sweep that only ran at startup would turn every provider hiccup into a
/// failed recording.
///
/// Silent when there is no runtime to spawn on. `web_router` is public and can
/// be built outside one; a router that serves without a sweeper is degraded,
/// and a panic at construction is worse.
pub(crate) fn spawn_recording_sweeper(
    accounts: Arc<Accounts>,
    recorder: crate::recording::Recorder,
) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        eprintln!("recording is configured but there is no runtime to sweep on");
        return;
    };
    handle.spawn(async move {
        loop {
            let outcome = crate::recording::sweep_recordings(&accounts, &recorder).await;
            if outcome != crate::recording::SweepOutcome::default() {
                eprintln!(
                    "recording sweep retried {} deleted {} failed {}",
                    outcome.retried, outcome.deleted, outcome.failed
                );
            }
            tokio::time::sleep(SWEEP_INTERVAL).await;
        }
    });
}

/// How often the delivery queue is asked whether anything is due.
///
/// Ten seconds, because the queue's own `run_after` is what schedules a retry
/// and this only decides how late the first attempt can be. A recording that
/// waits ten seconds for its upload to begin is a recording nobody noticed
/// waiting.
pub(crate) const DELIVERY_INTERVAL: Duration = Duration::from_secs(10);

/// The upload that a finished recording still owes.
///
/// Separate from the sweeper, because they answer different questions: the
/// sweeper looks for rows nothing is moving, and this moves them. Running them
/// on one timer would tie the pace of deliveries to the pace of a scan.
///
/// A deployment whose credentials do not build a delivery client gets a warning
/// and no worker rather than a panic in a router constructor. The binary does
/// not reach that case: `codetrial web` and `codetrial serve` parse the key
/// before they listen and refuse to start without one. It is reachable from
/// this function, which is public and is what the tests build, and there the
/// warning is the right answer.
/// The delivery client, or a warning and none.
///
/// A `None` here is a deployment that records and never hands anything over,
/// which the binary refuses to start in: `codetrial web` and `codetrial serve`
/// parse the key before they listen. It is reachable from the public router
/// constructors, which is what the tests build, and there a warning is the
/// right answer rather than a panic inside a constructor.
pub(crate) fn delivery_provider(
    recording: &crate::config::RecordingConfig,
) -> Option<Arc<dyn crate::recording::DeliveryProvider>> {
    match crate::delivery::GoogleDelivery::new(
        &recording.service_account_json,
        &recording.gcs_bucket,
        &recording.drive_id,
        Arc::new(|| crate::current_epoch_seconds() as i64),
    ) {
        Ok(delivery) => Some(Arc::new(delivery)),
        Err(error) => {
            eprintln!("WARNING: recordings cannot be delivered or deleted: {error}");
            None
        }
    }
}

pub(crate) fn spawn_delivery_worker(accounts: Arc<Accounts>, recorder: crate::recording::Recorder) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        eprintln!("recording is configured but there is no runtime to deliver on");
        return;
    };
    let Some(delivery) = recorder.delivery.clone() else {
        return;
    };
    handle.spawn(async move {
        loop {
            let outcome = crate::recording::run_delivery_queue(
                &accounts,
                &recorder,
                delivery.as_ref() as &dyn crate::recording::DeliveryProvider,
            )
            .await;
            if outcome != crate::recording::DeliveryOutcome::default() {
                eprintln!(
                    "recording delivery delivered {} retrying {} failed {}",
                    outcome.delivered, outcome.retrying, outcome.failed
                );
            }
            tokio::time::sleep(DELIVERY_INTERVAL).await;
        }
    });
}

/// Starts recording the room this interview claimed.
///
/// `202`, not `200`: the provider has been asked, the row says `starting`, and
/// whether an Egress job exists is something a webhook will say. Answering
/// `200` would promise a recording that may not have begun.
pub(crate) async fn start_recording_handler(
    State(state): State<AppState>,
    UriPath(interview_id): UriPath<String>,
    request: Request<Body>,
) -> Response {
    let (accounts, user) = match signed_in_owner(&state, request.headers()).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let Some(recorder) = state.recorder.clone() else {
        return json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_enabled", "error": "This server does not record interviews." }),
        );
    };

    // The address is read from the account rather than from the request, and
    // copied into the row: the file is delivered to whoever GitHub vouched for
    // at this moment, and the account's address can change afterwards.
    let Some(recipient) = user.verified_email.clone() else {
        return json_response(
            StatusCode::FORBIDDEN,
            json!({
                "code": "recording_requires_verified_identity",
                "error": "This interview is recorded, so it needs a GitHub sign-in."
            }),
        );
    };
    let Ok(recording_id) = random_token(16) else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not start recording." }),
        );
    };

    let begun = {
        let accounts = accounts.clone();
        let recorder = recorder.clone();
        let interview_id = interview_id.clone();
        let recording_id = recording_id.clone();
        blocking(move || {
            Ok(crate::recording::begin_recording(
                &accounts,
                recorder.clock.as_ref(),
                &interview_id,
                user.id,
                &recording_id,
                &recipient,
                recorder.config.kill_switch,
            ))
        })
        .await
    };
    let (recording, mine) = match begun {
        Ok(Ok(begun)) => begun,
        Ok(Err(refusal)) => return start_refusal_response(refusal),
        Err(_) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not start recording." }),
            );
        }
    };

    // Only the caller that inserted the row calls the provider. Two concurrent
    // starts both see a `starting` row with no egress id, and if both treated
    // that as work to do there would be two Egress jobs; a row whose creator
    // never got this far is the sweeper's.
    if !mine {
        return recording_state_now(&accounts, &recording).await;
    }

    if let Err(error) =
        crate::recording::start_and_record_egress(&accounts, &recorder, &recording).await
    {
        // Left in `starting`, not failed, and the row is not touched. The
        // contract promises one minute, then five, then fifteen; a row moved
        // straight to `failed` is a row that schedule never sees, and counting
        // this attempt as a retry would push the first retry out to five
        // minutes.
        eprintln!("recording {} could not start: {error}", recording.id);
    }
    recording_state_now(&accounts, &recording).await
}

/// The recording as the row has it now.
///
/// Re-read rather than reported from the snapshot this handler started with,
/// because the row moves underneath it: `starting` becomes `recording` when the
/// egress id lands, and a sweeper retry can have moved it first. Answering with
/// the older copy told the browser a recording had not begun when it had.
pub(crate) async fn recording_state_now(
    accounts: &Arc<Accounts>,
    recording: &crate::recording::Recording,
) -> Response {
    let fresh = {
        let accounts = accounts.clone();
        let recording_id = recording.id.clone();
        blocking(move || crate::recording::recording_by_id(&accounts, &recording_id)).await
    };
    match fresh {
        Ok(Some(fresh)) => recording_accepted(&fresh),

        // Not the snapshot. The whole point of this function is to answer with
        // what the row holds, and a state nobody read is exactly what the older
        // version reported by accident. The recording id comes back either way,
        // so a caller can ask again.
        Ok(None) | Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "code": "recording_state_unknown",
                "error": "The recording was started and its state could not be read.",
                "recordingId": recording.id
            }),
        ),
    }
}

pub(crate) fn recording_accepted(recording: &crate::recording::Recording) -> Response {
    json_response(
        StatusCode::ACCEPTED,
        json!({ "recordingId": recording.id, "state": recording.state.as_str() }),
    )
}

pub(crate) fn start_refusal_response(refusal: crate::recording::StartRefusal) -> Response {
    use crate::recording::StartRefusal;
    match refusal {
        StartRefusal::NoConsent => recording_requires_consent(),
        StartRefusal::NoRoom => json_response(
            StatusCode::CONFLICT,
            json!({
                "code": "recording_has_no_room",
                "error": "Join the interview before starting the recording."
            }),
        ),
        StartRefusal::KillSwitch => json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({
                "code": "recording_disabled",
                "error": "Recording is turned off on this server right now."
            }),
        ),
        StartRefusal::AlreadyActive => json_response(
            StatusCode::CONFLICT,
            json!({
                "code": "recording_already_active",
                "error": "This account already has an interview recording."
            }),
        ),
        StartRefusal::Database => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not start recording." }),
        ),
    }
}

/// What happened to this interview's recording.
///
/// Ids and states only. The staged object, the Drive file and the permission
/// are handles to media, and a status route that returned them would be a way
/// to reach a recording without the permission that governs it.
pub(crate) async fn recording_status_handler(
    State(state): State<AppState>,
    UriPath(interview_id): UriPath<String>,
    request: Request<Body>,
) -> Response {
    let (accounts, user) = match signed_in_owner(&state, request.headers()).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };

    // A server that records nothing answers exactly as one with no such
    // recording does. Three states, one reply: telling them apart is a way to
    // learn about other people's interviews and about this deployment.
    let missing = || {
        json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_found", "error": "No recording for that interview." }),
        )
    };
    if state.recorder.is_none() {
        return missing();
    }
    let found = blocking(move || {
        crate::recording::recording_for_interview(&accounts, &interview_id, user.id)
    })
    .await;
    match found {
        Ok(None) => missing(),
        Ok(Some(recording)) => {
            // Through `Failure::parse`, so only an enumerated code can reach a
            // client. The column is written by this crate today, and a status
            // route that echoed whatever was in it is one refactor away from
            // handing back a provider's error body.
            let failure = recording
                .error
                .as_deref()
                .and_then(crate::recording::Failure::parse);
            json_response(
                StatusCode::OK,
                json!({
                    "recordingId": recording.id,
                    "state": recording.state.as_str(),
                    "error": failure.map(crate::recording::Failure::as_str),

                    // What a person does about it. "failed" is not an
                    // instruction, and a candidate whose file exists and could
                    // not be delivered must not be told to record again.
                    "recovery": failure.map(crate::recording::Failure::recovery),
                    "retries": recording.retries,
                }),
            )
        }
        Err(error) => {
            eprintln!("could not read a recording status: {error}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read the recording status." }),
            )
        }
    }
}

/// Asks the provider to stop and moves the row to `finalizing`.
///
/// Idempotent by way of the transition table: `finalizing` may become itself,
/// so the second of the two normal-end paths to arrive changes nothing.
pub(crate) async fn finish_recording(
    _state: &AppState,
    accounts: &Arc<Accounts>,
    recorder: &crate::recording::Recorder,
    recording: &crate::recording::Recording,
    reason: Option<&str>,
) -> Response {
    use crate::recording::RecordingState;
    if !recording.state.is_active() {
        return recording_accepted(recording);
    }
    let next = if reason.is_some() {
        // A withdrawal is not a normal end. The recording is not finished, it
        // is abandoned, and the file it produced is scheduled for deletion.
        RecordingState::Failed
    } else {
        RecordingState::Finalizing
    };
    match crate::recording::stop_recording(accounts, recorder, recording, next, reason).await {
        Ok((state, _)) => json_response(
            StatusCode::ACCEPTED,
            json!({ "recordingId": recording.id, "state": state.as_str() }),
        ),
        Err(error) => {
            crate::recording::report_stop_failure(&recording.id, Some("end"), &error);
            json_response(
                StatusCode::BAD_GATEWAY,
                json!({
                    "code": "recording_stop_failed",
                    "error": "The recording could not be stopped.",
                    "recordingId": recording.id
                }),
            )
        }
    }
}

/// An account's own recordings, newest first.
///
/// Paged by cursor rather than by offset: an offset shifts under a row being
/// inserted or deleted, which here means an interview appearing twice or not at
/// all while somebody scrolls their own history.
pub(crate) async fn list_recordings_handler(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    request: Request<Body>,
) -> Response {
    let (accounts, user) = match signed_in_owner(&state, request.headers()).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    if state.recorder.is_none() {
        return json_response(
            StatusCode::OK,
            json!({ "recordings": [], "nextCursor": null }),
        );
    }

    // `<created_at>.<id>`, which is the last row of the previous page. Refused
    // rather than ignored when it does not parse: a cursor that quietly became
    // "start again" would hand a client the first page a second time, which
    // reads as a list that repeats rather than as a request that was wrong.
    let before = match params.get("before") {
        None => None,
        Some(cursor) => {
            let parsed = cursor.split_once('.').and_then(|(created_at, id)| {
                Some((created_at.parse::<i64>().ok()?, id.to_string()))
            });
            match parsed {
                Some(parsed) => Some(parsed),
                None => {
                    return json_response(
                        StatusCode::BAD_REQUEST,
                        json!({ "code": "cursor_invalid", "error": "That is not a page cursor." }),
                    );
                }
            }
        }
    };
    let page =
        blocking(move || crate::recording::recordings_for_account(&accounts, user.id, before))
            .await;
    let Ok(mut page) = page else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not read recordings." }),
        );
    };

    // One more than a page was asked for, which is how "is there another page"
    // is answered without a second count query.
    let more = page.len() as i64 > crate::recording::RECORDING_PAGE;
    page.truncate(crate::recording::RECORDING_PAGE as usize);
    let next = more
        .then(|| page.last())
        .flatten()
        .map(|last| format!("{}.{}", last.created_at, last.id));
    json_response(
        StatusCode::OK,
        json!({
            "recordings": page
                .iter()
                .map(|recording| json!({
                    "recordingId": recording.id,
                    "interviewId": recording.interview_id,
                    "state": recording.state.as_str(),
                    "error": recording
                        .error
                        .as_deref()
                        .and_then(crate::recording::Failure::parse)
                        .map(crate::recording::Failure::as_str),
                    "createdAt": recording.created_at,
                }))
                .collect::<Vec<_>>(),
            "nextCursor": next,
        }),
    )
}

/// One recording, as the person it belongs to may see it.
///
/// No object path, no Drive file id, no permission id. Those are handles to
/// media, and a history page needs to say whether a recording exists and until
/// when, not where it is kept.
pub(crate) async fn recording_handler(
    State(state): State<AppState>,
    UriPath(recording_id): UriPath<String>,
    request: Request<Body>,
) -> Response {
    let (accounts, user) = match signed_in_owner(&state, request.headers()).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let missing = || {
        json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_found", "error": "No such recording." }),
        )
    };
    if state.recorder.is_none() {
        return missing();
    }
    let found =
        blocking(move || crate::recording::recording_summary(&accounts, &recording_id, user.id))
            .await;
    let Ok(found) = found else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not read the recording." }),
        );
    };
    let Some(summary) = found else {
        return missing();
    };
    if let Some(gone) = gone_response(&summary, current_epoch_seconds() as i64) {
        return gone;
    }

    let failure = summary
        .error
        .as_deref()
        .and_then(crate::recording::Failure::parse);
    json_response(
        StatusCode::OK,
        json!({
            "recordingId": summary.id,
            "interviewId": summary.interview_id,
            "state": summary.state.as_str(),
            "error": failure.map(crate::recording::Failure::as_str),
            "recovery": failure.map(crate::recording::Failure::recovery),
            "createdAt": summary.created_at,
            "readyAt": summary.ready_at,
            "expiresAt": summary.expires_at,
            "quotaExceeded": summary.quota_exceeded,
        }),
    )
}

/// The replay of one recording, paged after a sequence number.
///
/// The same shape the recording template reads, and the same rules: absent or
/// negative `after` is the snapshot, a number is the tail.
pub(crate) async fn recording_events_handler(
    State(state): State<AppState>,
    UriPath(recording_id): UriPath<String>,
    Query(params): Query<HashMap<String, String>>,
    request: Request<Body>,
) -> Response {
    use crate::recording::SnapshotView;

    let (accounts, user) = match signed_in_owner(&state, request.headers()).await {
        Ok(owner) => owner,
        Err(response) => return response,
    };
    let missing = || {
        json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_found", "error": "No such recording." }),
        )
    };
    if state.recorder.is_none() {
        return missing();
    }
    let now = current_epoch_seconds() as i64;
    let after = params
        .get("after")
        .and_then(|after| after.parse::<i64>().ok())
        .unwrap_or(-1);

    let found = {
        let accounts = accounts.clone();
        let recording_id = recording_id.clone();
        blocking(move || crate::recording::recording_summary(&accounts, &recording_id, user.id))
            .await
    };
    let summary = match found {
        Ok(Some(summary)) => summary,
        Ok(None) => return missing(),

        // A read that failed is not a recording that does not exist. Answering
        // `404` for it tells a person their interview is gone when the database
        // merely had a bad moment.
        Err(error) => {
            eprintln!("could not read a recording: {error}");
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read the recording." }),
            );
        }
    };
    if let Some(gone) = gone_response(&summary, now) {
        return gone;
    }

    let view = blocking(move || {
        if after < 0 {
            crate::recording::replay_snapshot(&accounts, &summary.interview_id, user.id, now)
        } else {
            crate::recording::replay_tail(&accounts, &summary.interview_id, user.id, after, now)
        }
    })
    .await;
    match view {
        Ok(SnapshotView::Ready(snapshot)) => json_response(StatusCode::OK, replay_body(&snapshot)),
        Ok(SnapshotView::Expired) => json_response(
            StatusCode::GONE,
            json!({ "code": "replay_expired", "error": "This replay is past its retention deadline." }),
        ),
        Ok(SnapshotView::Deleted) => json_response(
            StatusCode::GONE,
            json!({ "code": "recording_deleted", "error": "This recording has been deleted." }),
        ),
        Ok(SnapshotView::NoInterview) => missing(),
        Err(error) => {
            eprintln!("could not read a replay: {error}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read the replay." }),
            )
        }
    }
}

/// `410` for a recording whose media is gone or past its deadline, and nothing
/// for one that is still there.
///
/// One function, because the two routes have to answer this the same way: a
/// history page that listed a recording as watchable and a replay that answered
/// `410` would be two answers to one question.
pub(crate) fn gone_response(
    summary: &crate::recording::RecordingSummary,
    now: i64,
) -> Option<Response> {
    if summary.deleted_at.is_some() {
        return Some(json_response(
            StatusCode::GONE,
            json!({ "code": "recording_deleted", "error": "This recording has been deleted." }),
        ));
    }
    if summary
        .expires_at
        .is_some_and(|expires_at| expires_at <= now)
    {
        return Some(json_response(
            StatusCode::GONE,
            json!({ "code": "replay_expired", "error": "This recording is past its retention deadline." }),
        ));
    }
    None
}

/// The replay, read by the page that is recording it.
///
/// The recording template holds no session cookie, only the join token Egress
/// minted for it, so this is the one replay read authorized by a room
/// credential rather than by an account. The token is signed with the project's
/// own secret, which this server holds, and it names the room it is good for:
/// that is the difference between the recorder of this room and anyone who
/// guessed a room name.
///
/// `after` chooses the shape. Absent or negative is the snapshot, which is the
/// replay with superseded frames dropped; a sequence number is everything after
/// it. One route, because the template's loop is the same call either way.
pub(crate) async fn recording_replay_handler(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    request: Request<Body>,
) -> Response {
    use crate::recording::SnapshotView;

    let Some(accounts) = state.accounts.clone() else {
        return state.accounts_error();
    };
    let Some(recorder) = state.recorder.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let refused = || {
        json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "room_token_invalid" }),
        )
    };
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let Some(key) = crate::token::livekit_token_issuer(&authorization) else {
        return refused();
    };
    let Some((api_key, api_secret)) = webhook_credentials(&state, &recorder, &authorization) else {
        return refused();
    };
    let now = recorder.clock.now().max(0);
    let Ok(room_name) =
        crate::token::livekit_room_from_token(&api_key, &api_secret, &authorization, now as u64)
    else {
        return refused();
    };

    // The same rule the webhook route applies: a credential from one project
    // must not read another project's room.
    if !signed_by_the_rooms_project(&state, &recorder, Some(&room_name), &key) {
        return refused();
    }

    let after = params
        .get("after")
        .and_then(|after| after.parse::<i64>().ok())
        .unwrap_or(-1);
    let found = {
        let accounts = accounts.clone();
        let room_name = room_name.clone();
        blocking(move || crate::recording::recording_for_room(&accounts, &room_name)).await
    };
    let Ok(Some(recording)) = found else {
        return json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_found", "error": "No recording for that room." }),
        );
    };

    let view = blocking(move || {
        if after < 0 {
            crate::recording::replay_snapshot(
                &accounts,
                &recording.interview_id,
                recording.account_id,
                now,
            )
        } else {
            crate::recording::replay_tail(
                &accounts,
                &recording.interview_id,
                recording.account_id,
                after,
                now,
            )
        }
    })
    .await;
    match view {
        Ok(SnapshotView::Ready(snapshot)) => json_response(StatusCode::OK, replay_body(&snapshot)),
        Ok(SnapshotView::Expired) => json_response(
            StatusCode::GONE,
            json!({ "code": "replay_expired", "error": "This replay is past its retention deadline." }),
        ),
        Ok(SnapshotView::Deleted) => json_response(
            StatusCode::GONE,
            json!({ "code": "recording_deleted", "error": "This recording has been deleted." }),
        ),
        Ok(SnapshotView::NoInterview) => json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_found", "error": "No replay for that room." }),
        ),
        Err(error) => {
            eprintln!("could not read a replay for a room: {error}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read the replay." }),
            )
        }
    }
}

/// LiveKit's webhooks.
///
/// Unauthenticated in the session sense and authenticated in every other: the
/// signature covers the body, and nothing in it is trusted until that checks
/// out. A refusal says nothing about which step failed, because telling a
/// caller "wrong secret" apart from "wrong digest" hands them a probe.
pub(crate) async fn recording_webhook_handler(
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    let Some(accounts) = state.accounts.clone() else {
        return state.accounts_error();
    };
    let Some(recorder) = state.recorder.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (parts, body) = request.into_parts();
    let Ok(body) = to_bytes(body, MAX_WEBHOOK_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let authorization = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    // The key names the project, and the project owns the secret. Reading it
    // from the token rather than assuming the primary project is what lets a
    // deployment with more than one LiveKit project verify a webhook from any
    // of them; the signature check below is still what decides.
    let Some((api_key, api_secret)) = webhook_credentials(&state, &recorder, authorization) else {
        eprintln!("recording webhook refused: unknown_key");
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "webhook_signature_invalid" }),
        );
    };
    if let Err(rejection) = crate::token::verify_livekit_webhook(
        &api_key,
        &api_secret,
        authorization,
        body.as_ref(),
        recorder.clock.now().max(0) as u64,
    ) {
        eprintln!("recording webhook refused: {rejection}");
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "webhook_signature_invalid" }),
        );
    }

    let Ok(event) = serde_json::from_slice::<Value>(body.as_ref()) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "webhook_body_invalid" }),
        );
    };
    let Some(event_id) = event.get("id").and_then(Value::as_str).map(str::to_string) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "webhook_body_invalid" }),
        );
    };

    // Claimed before anything is acted on. A retry is a fresh, validly signed
    // message with the same id, and acting on it twice is how one recording
    // gets stopped, transferred, or failed more than once.
    //
    // The ceiling: two identical deliveries arriving at once both find the
    // claim unapplied and both apply it. That is deliberate, because the
    // alternative is a lock held across a provider call, and every transition
    // below is idempotent by the table, so the outcome is the same one twice.
    let fresh = {
        let accounts = accounts.clone();
        let now = recorder.clock.now();
        blocking(move || crate::recording::claim_webhook_event(&accounts, &event_id, now)).await
    };
    match fresh {
        Ok(true) => {}

        // A duplicate is a success. LiveKit retries anything it did not get a
        // 2xx for, so answering anything else asks for it forever.
        Ok(false) => return StatusCode::OK.into_response(),
        Err(error) => {
            eprintln!("could not record a webhook event: {error}");
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "webhook_not_recorded" }),
            );
        }
    }

    if !apply_webhook(&state, &accounts, &recorder, &event, &api_key).await {
        // Not marked applied, so LiveKit's retry gets another attempt at it.
        // There is no compensating write here on purpose: the earlier version
        // deleted the claim, and a delete that itself failed lost the event.
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({ "error": "webhook_not_applied" }),
        );
    }

    let applied = {
        let accounts = accounts.clone();
        let event_id = event
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let now = recorder.clock.now();
        blocking(move || crate::recording::mark_webhook_applied(&accounts, &event_id, now)).await
    };
    if let Err(error) = applied {
        // The work is done and the ledger does not know. A redelivery would
        // repeat a transition that is idempotent by the table, so this is a
        // line rather than a failure.
        eprintln!("could not mark a webhook event applied: {error}");
    }
    StatusCode::OK.into_response()
}

/// The secret belonging to the key a webhook claims signed it.
///
/// `None` when no configured project holds that key, which is a refusal: a
/// message signed by a project this server does not serve is not this server's
/// message. Looking the key up rather than assuming the primary project is what
/// makes a multi-project deployment work at all, since a room belongs to one
/// project and the others' secrets verify nothing it signed.
///
/// There is deliberately no separate webhook secret. LiveKit signs with the
/// project's own API key and secret, so a third value would be one an operator
/// believed was checked.
pub(crate) fn webhook_credentials(
    state: &AppState,
    recorder: &crate::recording::Recorder,
    authorization: &str,
) -> Option<(String, String)> {
    let key = crate::token::livekit_token_issuer(authorization)?;
    if let Some(livekit) = &recorder.config.livekit
        && livekit.api_key == key
    {
        return Some((livekit.api_key.clone(), livekit.api_secret.clone()));
    }
    state
        .config
        .pool
        .providers
        .iter()
        .find(|provider| provider.api_key == key)
        .map(|provider| (provider.api_key.clone(), provider.api_secret.clone()))
}

/// Whether the project that signed a webhook is the one that owns this room.
///
/// The room name routes to a provider exactly as `/api/token` routed it, and
/// the key that signed the message has to be that provider's. The recording
/// override only substitutes a different key for the same project, so it is
/// checked against the room's provider rather than instead of it.
pub(crate) fn signed_by_the_rooms_project(
    state: &AppState,
    recorder: &crate::recording::Recorder,
    room_name: Option<&str>,
    signed_by: &str,
) -> bool {
    let Some(room_name) = room_name else {
        // A tombstoned recording has given its room name up, and there is
        // nothing left for a webhook to change.
        return false;
    };
    let Some(provider) = state
        .config
        .pool
        .for_room(room_name, &state.config.room_prefix)
    else {
        return false;
    };

    // Either key for that project. The override is a different key for the same
    // project, not a different project, and LiveKit signs a webhook with
    // whichever key the project's webhook configuration names, which is not
    // necessarily the one this server makes Egress calls with.
    if provider.api_key == signed_by {
        return true;
    }
    recorder.config.livekit.as_ref().is_some_and(|livekit| {
        livekit.api_key == signed_by
            && crate::config::same_livekit_project(&livekit.url, &provider.url)
    })
}

/// Whether the event was dealt with.
///
/// `false` asks the caller to give the claim back so LiveKit delivers it again.
/// The case that matters is an egress event arriving before this server has
/// written down the egress id it names: dropping it there would lose the only
/// notice that a recording started, ended, or failed.
pub(crate) async fn apply_webhook(
    state: &AppState,
    accounts: &Arc<Accounts>,
    recorder: &crate::recording::Recorder,
    event: &Value,
    signed_by: &str,
) -> bool {
    use crate::recording::RecordingState;
    let kind = event
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or_default();

    // lowerCamelCase, unlike the Twirp call that started this: upstream
    // marshals the webhook with protojson directly. The contract document is
    // where that asymmetry is explained.
    let egress_id = event
        .pointer("/egressInfo/egressId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let status = event
        .pointer("/egressInfo/status")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let found = match kind {
        "room_finished" => {
            let room = event
                .pointer("/room/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let accounts = accounts.clone();
            blocking(move || crate::recording::recording_for_room(&accounts, &room)).await
        }
        "egress_started" | "egress_updated" | "egress_ended" => {
            let Some(egress_id) = egress_id else {
                return true;
            };
            let accounts = accounts.clone();
            blocking(move || crate::recording::recording_for_egress(&accounts, &egress_id)).await
        }

        // Acknowledged and dropped. Answering anything else asks LiveKit to
        // keep sending events this pipeline has no opinion about.
        _ => return true,
    };
    let recording = match found {
        Ok(Some(recording)) => recording,

        // No recording for this event. For a room that is not this server's,
        // that is the end of it; for an egress event it usually means the start
        // response has not been written down yet, and this is the only notice
        // that job will ever get.
        Ok(None) => return kind == "room_finished",
        Err(error) => {
            eprintln!("could not read a recording for a webhook: {error}");
            return false;
        }
    };

    // A valid signature proves which project sent this, not which project owns
    // the room it names. Without this, any configured project could end another
    // project's recording by naming its room.
    if !signed_by_the_rooms_project(state, recorder, recording.room_name.as_deref(), signed_by) {
        eprintln!(
            "recording {} was named by a webhook from another project",
            recording.id
        );
        return true;
    }

    match kind {
        "room_finished" => finish_recording(state, accounts, recorder, &recording, None)
            .await
            .status()
            .is_success(),
        "egress_started" => {
            let accounts = accounts.clone();
            let clock = recorder.clock.clone();
            let id = recording.id.clone();
            blocking(move || {
                crate::recording::transition(
                    &accounts,
                    clock.as_ref(),
                    &id,
                    RecordingState::Recording,
                    None,
                )
            })
            .await
            .is_ok()
        }
        "egress_ended" | "egress_updated" => {
            apply_egress_status(accounts, recorder, &recording, event, status).await
        }
        _ => true,
    }
}

/// What the provider's own word for how a job ended means for the row.
///
/// `None` is an update that says nothing new. `EGRESS_ACTIVE` and
/// `EGRESS_STARTING` in particular do not say the job has stopped, and marking
/// it stopped here would tell the orphan sweep to stop watching a job that is
/// still running.
pub(crate) fn egress_outcome(
    status: &str,
    event: &Value,
    expected_object: &str,
) -> Option<(
    crate::recording::RecordingState,
    Option<crate::recording::Failure>,
)> {
    use crate::recording::{Failure, RecordingState};
    let outcome = match status {
        // A completion with nothing in it is not a completion. A missing file
        // result, or one of no bytes, is how a truncated recording arrives, and
        // sending it through the transfer shared a zero-byte video with a
        // candidate.
        "EGRESS_COMPLETE" if crate::recording::completed_with_output(event, expected_object) => {
            (RecordingState::Transferring, None)
        }
        "EGRESS_COMPLETE" => (RecordingState::Failed, Some(Failure::PartialOutput)),
        "EGRESS_FAILED" => (RecordingState::Failed, Some(Failure::Egress)),
        "EGRESS_ABORTED" => (RecordingState::Failed, Some(Failure::Aborted)),
        "EGRESS_LIMIT_REACHED" => (RecordingState::Failed, Some(Failure::LimitReached)),
        _ => return None,
    };
    Some(outcome)
}

/// Applies one `egress_ended` or `egress_updated` to the row it names.
pub(crate) async fn apply_egress_status(
    accounts: &Arc<Accounts>,
    recorder: &crate::recording::Recorder,
    recording: &crate::recording::Recording,
    event: &Value,
    status: &str,
) -> bool {
    use crate::recording::RecordingState;

    // The object this recording is going to transfer, not any file the job
    // happened to write.
    let expected_object =
        crate::recording::gcs_object_path(&recorder.config.gcs_prefix, &recording.id);
    let Some((next, failure)) = egress_outcome(status, event, &expected_object) else {
        return true;
    };

    // Terminal, whichever way it ended, so the row stops being one the sweeper
    // has to chase. A failure here fails the whole application, because the
    // alternative is a terminal row that looks stopped and a job that is not.
    let stopped = {
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let id = recording.id.clone();
        blocking(move || crate::recording::mark_stopped(&accounts, clock.as_ref(), &id)).await
    };
    if stopped.is_err() {
        return false;
    }
    let moved = {
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let id = recording.id.clone();
        let reason = failure.map(|failure| failure.as_str().to_string());
        blocking(move || {
            crate::recording::transition(&accounts, clock.as_ref(), &id, next, reason.as_deref())
        })
        .await
    };

    // Queued only when this call moved the row, and after the move rather than
    // before it. A webhook LiveKit sent twice must not become two deliveries,
    // and the queue's own `ON CONFLICT` is the second half of that rather than
    // the first.
    if next == RecordingState::Transferring
        && let Ok(Some((_, true))) = &moved
    {
        let accounts = accounts.clone();
        let id = recording.id.clone();
        let now = recorder.clock.now();
        if let Err(error) =
            blocking(move || crate::recording::enqueue_delivery(&accounts, &id, now)).await
        {
            // Not fatal to the webhook: the row is `transferring` and the
            // sweeper will not lose it. It does mean nobody has queued the
            // delivery, which is worth a line.
            eprintln!("WARNING: could not queue a delivery: {error}");
        }
    }

    // After the transition, and only when it moved. A late `EGRESS_FAILED` for
    // a row that has already advanced is refused by the table, and an audit
    // line written first said a recording had failed while its status said
    // otherwise.
    if let (Some(failure), Ok(Some((_, true)))) = (failure, &moved) {
        crate::recording::audit(
            "recording_failed",
            &recording.id,
            &[
                ("reason", failure.as_str()),
                ("recovery", failure.recovery()),
            ],
        );
    }
    moved.is_ok()
}

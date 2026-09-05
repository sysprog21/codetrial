//! An account's own recordings, over HTTP: starting and stopping an Egress
//! job, and the queries that list and serve the result.
//!
//! Two regions are out. `workers` holds the sweeper and the delivery worker,
//! which no request drives, and `webhook` holds LiveKit's callback, which
//! authenticates on a signature rather than a session.
//!
//! What is left is one caller and one auth model: every route here takes
//! `Owner`, answers the account that owns the recording, and shares
//! `recording_missing` and `gone_response`. Starting a recording and reading
//! one back looked like two regions by line count and are not: splitting them
//! would put a file boundary through one surface rather than along a seam.

mod webhook;
mod workers;

pub(crate) use webhook::recording_webhook_handler;

// Not re-exported: the replay route below is the only caller outside `webhook`,
// so these stay visible to this module and no wider.
use webhook::{signed_by_the_rooms_project, webhook_credentials};
pub(crate) use workers::{delivery_provider, spawn_delivery_worker, spawn_recording_sweeper};

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{Path as UriPath, Query, State};
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::accounts::{Accounts, blocking, random_token};
use crate::current_epoch_seconds;

use super::auth::Owner;
use super::consent::recording_requires_consent;
use super::{AppState, json_response, rate_limited_response, replay_response};

/// What one account may spend reading its own recordings in a window.
///
/// Derived from the first-party caller rather than picked, the way
/// `REPLAY_RATE_LIMIT` is. `web/replay.js` reads the listing once a page load
/// and then the detail and the events of whichever recording a person clicks,
/// so a reviewer working quickly through a history spends single digits a
/// minute against this budget. A hundred and twenty leaves that untouched with
/// room to spare.
///
/// The reads it bounds are cheap per call and not free per row: the review read
/// added for the response window returns every `avatar` and `lifecycle`
/// transition rather than the newest of each, which is what made "nothing
/// bounds how often" worth closing rather than noting.
pub const READ_RATE_LIMIT: u32 = 120;

/// Whether this account may spend one more read, in the words the routes behind
/// it answer with: the listing, the detail and the events.
///
/// Keyed on the account, which `Owner` has already resolved, for the reason the
/// ingest limiter gives: the address would put a whole office behind one budget
/// and let one looping tab refuse everyone beside it.
///
/// Not every read in this file is behind it. The room-token replay route has no
/// account to charge, and the status route that `web/recording-state.js` polls
/// is not budgeted.
fn read_allowed(state: &AppState, account_id: i64) -> Option<Response> {
    (!state.read_limit.allow(account_id, Instant::now()))
        .then(|| rate_limited_response("Too many reads at once. Wait a minute."))
}

/// The `after` query parameter, which chooses the shape of a replay read.
///
/// Absent, negative or unparseable is the whole replay; a sequence number is
/// everything past it. Both replay routes take it, so the sentinel is spelled
/// once: a second spelling is a second thing to keep in step with `ReplayRead`,
/// which is what actually decides.
fn replay_after(params: &HashMap<String, String>) -> i64 {
    params
        .get("after")
        .and_then(|after| after.parse::<i64>().ok())
        .unwrap_or(-1)
}

/// Starts recording the room this interview claimed.
///
/// `202`, not `200`: the provider has been asked, the row says `starting`, and
/// whether an Egress job exists is something a webhook will say. Answering
/// `200` would promise a recording that may not have begun.
pub(crate) async fn start_recording_handler(
    State(state): State<AppState>,
    UriPath(interview_id): UriPath<String>,
    Owner { accounts, user }: Owner,
) -> Response {
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
async fn recording_state_now(
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

fn recording_accepted(recording: &crate::recording::Recording) -> Response {
    json_response(
        StatusCode::ACCEPTED,
        json!({ "recordingId": recording.id, "state": recording.state.as_str() }),
    )
}

fn start_refusal_response(refusal: crate::recording::StartRefusal) -> Response {
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
    Owner { accounts, user }: Owner,
) -> Response {
    // A server that records nothing answers exactly as one with no such
    // recording does. Three states, one reply: telling them apart is a way to
    // learn about other people's interviews and about this deployment. Its own
    // sentence: this route is asked about an interview, and the two others
    // about a recording id, so the words differ where the code does not.
    let no_recording = || {
        json_response(
            StatusCode::NOT_FOUND,
            json!({ "code": "recording_not_found", "error": "No recording for that interview." }),
        )
    };
    if state.recorder.is_none() {
        return no_recording();
    }
    let found = blocking(move || {
        crate::recording::recording_for_interview(&accounts, &interview_id, user.id)
    })
    .await;
    match found {
        Ok(None) => no_recording(),
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
    Owner { accounts, user }: Owner,
) -> Response {
    if state.recorder.is_none() {
        return json_response(
            StatusCode::OK,
            json!({ "recordings": [], "nextCursor": null }),
        );
    }
    if let Some(refused) = read_allowed(&state, user.id) {
        return refused;
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
    Owner { accounts, user }: Owner,
) -> Response {
    if state.recorder.is_none() {
        return recording_missing();
    }
    if let Some(refused) = read_allowed(&state, user.id) {
        return refused;
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
        return recording_missing();
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
///
/// `avatar=history` is the one thing this route reads that the template's does
/// not. It answers the snapshot with the collapse narrowed to `Restates`, so
/// the transition histories survive; `Collapse` in `src/recording/replay.rs`
/// carries what each read is for and why the template gets neither history.
///
/// It applies to the snapshot only. The tail collapses nothing already, so the
/// parameter has nothing to lift there, and a request carrying both is answered
/// as the tail it asked for.
pub(crate) async fn recording_events_handler(
    State(state): State<AppState>,
    UriPath(recording_id): UriPath<String>,
    Query(params): Query<HashMap<String, String>>,
    Owner { accounts, user }: Owner,
) -> Response {
    if state.recorder.is_none() {
        return recording_missing();
    }
    if let Some(refused) = read_allowed(&state, user.id) {
        return refused;
    }
    let now = current_epoch_seconds() as i64;
    let after = replay_after(&params);

    // Exact, not truthy. An unknown value is the ordinary snapshot rather than
    // a guess at what the caller meant, so a typo reads one avatar row instead
    // of quietly widening the answer.
    let avatar_history = params.get("avatar").map(String::as_str) == Some("history");

    let found = {
        let accounts = accounts.clone();
        let recording_id = recording_id.clone();
        blocking(move || crate::recording::recording_summary(&accounts, &recording_id, user.id))
            .await
    };
    let summary = match found {
        Ok(Some(summary)) => summary,
        Ok(None) => return recording_missing(),

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

    // After `gone_response`, and `replay_view` checks expiry and deletion again
    // inside its own transaction. Both guards cover the review read for the
    // same reason they cover the snapshot: it is the same function with one
    // argument changed, so there is no second path to leave unguarded.
    let view = blocking(move || {
        if after >= 0 {
            crate::recording::replay_tail(&accounts, &summary.interview_id, user.id, after, now)
        } else if avatar_history {
            crate::recording::replay_review(&accounts, &summary.interview_id, user.id, now)
        } else {
            crate::recording::replay_snapshot(&accounts, &summary.interview_id, user.id, now)
        }
    })
    .await;
    replay_response(view, recording_missing, "could not read a replay")
}

/// No such recording, in the words the detail and the events routes both use.
///
/// Written out twice before, which is two chances for one of them to start
/// saying something a caller can tell apart from the other: an account that
/// does not own a recording must not learn that it exists, and that only holds
/// while both routes answer a stranger and an owner of nothing the same way.
///
/// The other two routes in this file keep their own wording deliberately, and
/// each says why where it says it: one is asked about an interview and one
/// about a room.
fn recording_missing() -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        json!({ "code": "recording_not_found", "error": "No such recording." }),
    )
}

/// `410` for a recording whose media is gone or past its deadline, and nothing
/// for one that is still there.
///
/// One function, because the two routes have to answer this the same way: a
/// history page that listed a recording as watchable and a replay that answered
/// `410` would be two answers to one question.
fn gone_response(summary: &crate::recording::RecordingSummary, now: i64) -> Option<Response> {
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

    let after = replay_after(&params);
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
    replay_response(
        view,
        || {
            json_response(
                StatusCode::NOT_FOUND,
                json!({ "code": "recording_not_found", "error": "No replay for that room." }),
            )
        },
        "could not read a replay for a room",
    )
}

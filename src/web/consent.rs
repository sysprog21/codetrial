//! Recording consent: the candidate's grant, its withdrawal, and the two checks
//! the token and start-recording paths run before a room may be captured.
//!
//! Its own module rather than a corner of `interviews` because all three of
//! those callers need it, and keeping it inside any one of them made the other
//! two reach sideways into a sibling.

use std::sync::Arc;

use axum::extract::{Path as UriPath, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::accounts::{
    Accounts, SignedInUser, blocking, claim_interview_room, interview_for_account, withdraw_consent,
};
use crate::current_epoch_seconds;

use super::auth::Owner;
use super::{AppState, json_response};

/// Takes consent back.
///
/// 204 and no body, because there is nothing to say: the interview is the
/// candidate's, the withdrawal is recorded, and what happens to an active
/// recording is the recording lifecycle's problem. This route owns the state it
/// requests, not the transition.
pub(crate) async fn withdraw_consent_handler(
    State(state): State<AppState>,
    UriPath(interview_id): UriPath<String>,
    Owner { accounts, user }: Owner,
) -> Response {
    let now = current_epoch_seconds() as i64;
    let withdrawn = {
        let accounts = accounts.clone();
        let interview_id = interview_id.clone();
        blocking(move || withdraw_consent(&accounts, &interview_id, user.id, now)).await
    };
    match withdrawn {
        // Not found and somebody else's are one answer. Telling them apart is a
        // way to enumerate other people's interview ids.
        Ok(false) => json_response(
            StatusCode::NOT_FOUND,
            json!({ "error": "No such interview." }),
        ),
        Ok(true) => {
            // The row is written first and the provider is told second, so a
            // failed stop leaves a withdrawal that is recorded and a recording
            // the sweeper will find. The other order loses the withdrawal.
            if let Some(recorder) = state.recorder.clone() {
                let found = {
                    let accounts = accounts.clone();
                    blocking(move || {
                        crate::recording::recording_for_interview(&accounts, &interview_id, user.id)
                    })
                    .await
                };
                if let Ok(Some(recording)) = found
                    && recording.state.is_active()
                    && let Err(error) = crate::recording::stop_recording(
                        &accounts,
                        &recorder,
                        &recording,
                        crate::recording::RecordingState::Failed,
                        Some("consent_withdrawn"),
                    )
                    .await
                {
                    // Still 204. The candidate said no and that is recorded;
                    // whether the provider heard it is the sweeper's problem,
                    // not theirs. Through `audit`, because the message is the
                    // provider's and a newline in it would forge a log line.
                    crate::recording::report_stop_failure(
                        &recording.id,
                        Some("consent_withdrawn"),
                        &error,
                    );
                }
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(error) => {
            eprintln!("could not withdraw interview consent: {error}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not withdraw consent." }),
            )
        }
    }
}

pub(crate) fn recording_requires_consent() -> Response {
    json_response(
        StatusCode::FORBIDDEN,
        json!({
            "code": "recording_requires_consent",
            "error": "This interview is recorded. Agree to the recording notice before starting."
        }),
    )
}

/// The consent a recorded room may start under, or the response refusing it.
///
/// Returns `Ok(None)` where recording is off, which is the state every
/// deployment is in until an operator provisions it: there is no consent to
/// check because there is nothing to consent to.
/// The interview id a recorded room may start under, checked but not yet spent.
///
/// Split from the claim below on purpose. This runs before the token is signed
/// and before an interviewer is dispatched, so a request that is going to be
/// refused is refused cheaply; the claim runs after both, so a failure between
/// them does not burn a candidate's only consent and leave them unable to
/// retry.
///
/// `Ok(None)` where recording is off, which is the state every deployment is in
/// until an operator provisions it: there is no consent to check because there
/// is nothing to consent to.
#[allow(clippy::result_large_err)] // Responses are immediately returned by HTTP handlers.
pub(crate) async fn checked_consent(
    state: &AppState,
    accounts: &Arc<Accounts>,
    user: &SignedInUser,
    body: &[u8],
) -> Result<Option<String>, Response> {
    if state.config.recording.is_none() {
        return Ok(None);
    }
    let Some(interview_id) = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|body| body.get("interviewId")?.as_str().map(str::to_string))
        .filter(|id| !id.is_empty())
    else {
        return Err(recording_requires_consent());
    };
    let owner = user.id;
    let lookup = {
        let accounts = accounts.clone();
        let interview_id = interview_id.clone();
        blocking(move || interview_for_account(&accounts, &interview_id, owner)).await
    };
    match lookup {
        Ok(Some(interview))
            if interview.consent_withdrawn_at.is_none() && interview.room_name.is_none() =>
        {
            Ok(Some(interview.id))
        }
        Ok(_) => Err(recording_requires_consent()),
        Err(error) => {
            eprintln!("could not read interview consent: {error}");
            Err(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read interview consent." }),
            ))
        }
    }
}

/// Spends the consent on one room, or refuses.
///
/// One statement, and it answers every refusal that matters here: gone,
/// somebody else's, withdrawn, or already used. The read above is a
/// convenience; this is the decision, because two token requests racing on one
/// interview must not both win and a read followed by a write is exactly the
/// gap where they would.
///
/// The ceiling, because it is not obvious: this closes the window between
/// checking consent and minting a token, and it does not close the one between
/// minting a token and starting Egress. A candidate who withdraws in that
/// second window has a room they may join and a recording that must not begin,
/// which is the recording lifecycle's problem: the authoritative check runs
/// immediately before the provider call.
#[allow(clippy::result_large_err)] // Responses are immediately returned by HTTP handlers.
pub(crate) async fn claim_consent(
    accounts: &Arc<Accounts>,
    user_id: i64,
    interview_id: String,
    room_name: &str,
) -> Result<(), Response> {
    let accounts = accounts.clone();
    let room_name = room_name.to_string();
    match blocking(move || claim_interview_room(&accounts, &interview_id, user_id, &room_name))
        .await
    {
        Ok(true) => Ok(()),
        Ok(false) => Err(recording_requires_consent()),
        Err(error) => {
            eprintln!("could not claim interview consent: {error}");
            Err(json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not read interview consent." }),
            ))
        }
    }
}

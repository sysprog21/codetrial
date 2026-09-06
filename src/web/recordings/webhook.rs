//! LiveKit's webhook, and what one is allowed to change.
//!
//! The only entry point in `recordings` a browser never reaches. It answers
//! LiveKit rather than a candidate, so it authenticates on a signature instead
//! of a session, and everything below the handler exists to bound what a
//! verified caller may then touch: `signed_by_the_rooms_project` ties the key
//! that signed to the project owning the room, so a webhook authenticated for
//! one project cannot act on another's recording.
//!
//! Split out of `recordings.rs` for that reason rather than for its size. A
//! trust boundary is easier to review as a file than as the last third of one.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::accounts::{Accounts, blocking};

use super::super::{AppState, MAX_WEBHOOK_BODY_BYTES, json_response};
use super::finish_recording;

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
pub(super) fn webhook_credentials(
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
pub(super) fn signed_by_the_rooms_project(
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
async fn apply_webhook(
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
fn egress_outcome(
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
async fn apply_egress_status(
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

    // Only while the provider still owns the row. Its news can arrive late and
    // out of order, and `transferring` to `failed` is a move the table has to
    // allow for the delivery worker, so an `EGRESS_FAILED` retried after a
    // completed egress could fail a recording whose file was already queued and
    // whole. Once the transfer side has the row, the provider has nothing left
    // to say about it.
    const PROVIDER_OWNED: [RecordingState; 3] = [
        RecordingState::Starting,
        RecordingState::Recording,
        RecordingState::Finalizing,
    ];
    let moved = {
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let id = recording.id.clone();
        let reason = failure.map(|failure| failure.as_str().to_string());
        blocking(move || {
            crate::recording::transition_within(
                &accounts,
                clock.as_ref(),
                &id,
                Some(&PROVIDER_OWNED),
                next,
                reason.as_deref(),
            )
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
    // a row that has already advanced is refused above, and an audit line
    // written first said a recording had failed while its status said
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

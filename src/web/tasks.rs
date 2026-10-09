//! Account-bound task unlock and public exercise loading.

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::SocketAddr;

use crate::tasks::access::{
    AccessError, Admission, Assignment, Preparation, ResultLookup, TaskService,
};

use super::auth::Owner;
use super::{AppState, MAX_BODY_BYTES, json_response};

pub(crate) fn task_error(error: AccessError) -> Response {
    let (status, retryable, message) = match error.code {
        "credentials_missing" => (
            StatusCode::PRECONDITION_FAILED,
            false,
            "This CodeTrial has no Gemini API key, so no interviewer can join. Add GOOGLE_API_KEY to its configuration and restart it.",
        ),
        "loopback_required" => (
            StatusCode::FORBIDDEN,
            false,
            "Task mode runs only when CodeTrial listens on this computer alone.",
        ),
        "authentication_required" => (StatusCode::UNAUTHORIZED, false, "Sign in with GitHub."),
        "csrf_rejected" => (StatusCode::FORBIDDEN, false, "Reload the task page."),
        "site_invalid" => (
            StatusCode::BAD_REQUEST,
            false,
            "The assignment link names no valid HTTPS site.",
        ),
        "invalid_pin" => (StatusCode::FORBIDDEN, true, "The PIN is incorrect."),
        "unlock_required" => (
            StatusCode::FORBIDDEN,
            false,
            "Enter the PIN for this task set.",
        ),
        "package_unavailable" => (
            StatusCode::SERVICE_UNAVAILABLE,
            true,
            "The task set could not be downloaded.",
        ),
        "package_invalid" => (
            StatusCode::UNPROCESSABLE_ENTITY,
            false,
            "The published task set is damaged; ask your instructor.",
        ),
        "set_closed" => (StatusCode::GONE, false, "This task set is closed."),
        "task_not_found" => (StatusCode::NOT_FOUND, false, "This task is unavailable."),
        "rules_unacknowledged" => (
            StatusCode::FORBIDDEN,
            false,
            "Read and accept the rules before starting.",
        ),
        "calibration_required" => (
            StatusCode::FORBIDDEN,
            true,
            "Run the camera calibration before starting.",
        ),
        "client_override" => (
            StatusCode::BAD_REQUEST,
            false,
            "Task settings come from the task set.",
        ),
        "language_unsupported" => (
            StatusCode::BAD_REQUEST,
            false,
            "Choose one of this task's languages.",
        ),
        "language_unavailable" => (
            StatusCode::PRECONDITION_FAILED,
            false,
            "This task's languages compile on Compiler Explorer, which CODETRIAL_COMPILER_EXPLORER_ENABLED has turned off.",
        ),
        "platform_unsupported" => (
            StatusCode::BAD_REQUEST,
            false,
            "Use desktop Chromium or Firefox.",
        ),
        "request_key_expired" => (StatusCode::CONFLICT, false, "Start again."),
        "setup_pending" => (StatusCode::CONFLICT, true, "The task is still connecting."),
        "admission_throttled" => (
            StatusCode::TOO_MANY_REQUESTS,
            true,
            "Too many starts; wait a minute, then start again.",
        ),
        "request_too_large" => (
            StatusCode::PAYLOAD_TOO_LARGE,
            false,
            "The request is too large.",
        ),
        "active_task_session" => (
            StatusCode::CONFLICT,
            false,
            "Finish the active task before starting another.",
        ),
        "result_unavailable" => (
            StatusCode::NOT_FOUND,
            false,
            "No result is held for that attempt.",
        ),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            true,
            "The task could not connect.",
        ),
    };
    // `json_response` already marks every answer `no-store`.
    let mut response = json_response(
        status,
        json!({"code": error.code, "retryable": retryable, "message": message}),
    );
    if let Some(seconds) = error.retry_after {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, seconds.to_string().parse().unwrap());
    }
    response
}

/// Task mode is off unless the server listens on loopback alone; the routes
/// still answer, with that reason. A request must also name a loopback host:
/// a page served under another name, by a proxy nobody declared or a DNS
/// name rebound to this machine, is not the learner at this computer.
fn service<'a>(state: &'a AppState, headers: &HeaderMap) -> Option<&'a TaskService> {
    state
        .config
        .tasks
        .as_ref()
        .filter(|_| super::setup::host_names_loopback(headers.get(header::HOST)))
}

/// The address an assignment link opens; the one place it is spelled.
pub(crate) const TASK_PAGE_ROUTE: &str = "/t/{set_id}/{task_id}";

/// The set and task an assignment page's path names, when it is one.
pub(crate) fn task_page(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/t/")?;
    let (set_id, task_id) = rest.split_once('/')?;
    (crate::tasks::identifier(set_id) && crate::tasks::record_id(task_id))
        .then_some((set_id, task_id))
}

/// Where a sign-in may return to: an assignment page, its link's site and
/// version checked as `Assignment::new` checks them, rebuilt from those
/// parts so nothing else in the original survives. Exactly one `site` and one
/// `version`, or none.
pub(crate) fn task_return_path(path: &str) -> Option<String> {
    if path.len() > 4096 || path.contains('#') || path.chars().any(|c| c.is_control()) {
        return None;
    }
    let (pathname, query) = path.split_once('?').unwrap_or((path, ""));
    let (set_id, _) = task_page(pathname)?;
    if query.is_empty() {
        return Some(pathname.to_owned());
    }

    // Parsed as a list, not a map, so a repeated key is refused rather than one
    // copy of it silently winning.
    let query_url = reqwest::Url::parse(&format!("http://localhost/?{query}")).ok()?;
    let pairs: Vec<_> = query_url.query_pairs().collect();
    let value = |key: &str| {
        pairs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_ref())
    };
    let (site, version) = (value("site")?, value("version")?.parse::<u32>().ok()?);
    if pairs.len() != 2 {
        return None;
    }
    Assignment::new(site, set_id, version).ok()?;
    Some(format!(
        "{pathname}?site={}&version={version}",
        crate::percent_encode_component(site)
    ))
}

/// Why a device sign-in request is refused, in the task error shape the page
/// recovers from: the page's own header missing, or a host that does not name
/// this machine.
pub(crate) fn device_refusal(headers: &HeaderMap) -> Option<Response> {
    if !task_csrf(headers) {
        Some(task_error(AccessError::new("csrf_rejected")))
    } else if !super::setup::host_names_loopback(headers.get(header::HOST)) {
        Some(task_error(AccessError::new("loopback_required")))
    } else {
        None
    }
}

/// The signed-in account, refused in the task error shape: the interview
/// routes' refusal carries no `code`, and the page would offer no sign-in.
pub(crate) struct TaskOwner(Owner);

impl axum::extract::FromRequestParts<AppState> for TaskOwner {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Response> {
        match Owner::from_request_parts(parts, state).await {
            Ok(owner) => Ok(Self(owner)),
            Err(refusal) if refusal.status() == StatusCode::UNAUTHORIZED => {
                Err(task_error(AccessError::new("authentication_required")))
            }
            Err(refusal) => Err(refusal),
        }
    }
}

fn task_mode_off() -> Response {
    task_error(AccessError::new("loopback_required"))
}

pub(crate) async fn task_result_handler(
    State(state): State<AppState>,
    Path(room): Path<String>,
    headers: HeaderMap,
    TaskOwner(owner): TaskOwner,
) -> Response {
    let Some(service) = service(&state, &headers) else {
        return task_mode_off();
    };
    match service.result(owner.user.id, &room, crate::current_epoch_seconds()) {
        ResultLookup::Ready(result) => json_response(StatusCode::OK, result),
        ResultLookup::Pending => json_response(StatusCode::ACCEPTED, json!({"pending": true})),
        ResultLookup::Unavailable => task_error(AccessError::new("result_unavailable")),
    }
}

/// Whether a state-changing task request came from the task page itself.
///
/// The custom header is what carries it: a cross-origin page cannot send one
/// without a CORS preflight, and this server answers none. `Sec-Fetch-Site`
/// refuses a cross-site request from a browser that reports it. A same-origin
/// page under another name, a DNS name rebound to this machine, is refused
/// by the loopback `Host` check in `service` instead.
pub(crate) fn task_csrf(headers: &HeaderMap) -> bool {
    if headers
        .get("x-codetrial-task-request")
        .and_then(|v| v.to_str().ok())
        != Some("1")
    {
        return false;
    }
    !headers
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|site| !matches!(site, "same-origin" | "none"))
}

/// The signed-in learner's GitHub login and numeric id, as their result
/// records them.
fn task_github(user: &crate::accounts::SignedInUser) -> Option<(String, u64)> {
    Some((user.login.clone(), user.github_id?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnlockRequest {
    site: String,
    version: u32,
    pin: String,
}

pub(crate) async fn task_unlock_handler(
    State(state): State<AppState>,
    Path(set_id): Path<String>,
    TaskOwner(owner): TaskOwner,
    request: Request<Body>,
) -> Response {
    let Some(service) = service(&state, request.headers()) else {
        return task_mode_off();
    };
    if !task_csrf(request.headers()) {
        return task_error(AccessError::new("csrf_rejected"));
    }
    if task_github(&owner.user).is_none() {
        return task_error(AccessError::new("authentication_required"));
    }
    let Ok(bytes) = to_bytes(request.into_body(), MAX_BODY_BYTES).await else {
        return task_error(AccessError::new("request_too_large"));
    };
    let Ok(body) = serde_json::from_slice::<UnlockRequest>(&bytes) else {
        return task_error(AccessError::new("site_invalid"));
    };
    let assignment = match Assignment::new(&body.site, &set_id, body.version) {
        Ok(assignment) => assignment,
        Err(error) => return task_error(error),
    };
    match service
        .unlock(
            owner.user.id,
            &assignment,
            &body.pin,
            crate::current_epoch_seconds(),
        )
        .await
    {
        Ok(value) => json_response(StatusCode::OK, value),
        Err(error) => task_error(error),
    }
}

/// The site and set version a task is loaded from: the assignment link names
/// both.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssignmentQuery {
    site: String,
    version: u32,
}

pub(crate) async fn task_load_handler(
    State(state): State<AppState>,
    Path((set_id, task_id)): Path<(String, String)>,
    headers: HeaderMap,

    // After the session, so an anonymous request is told to sign in before
    // anything about its query.
    TaskOwner(owner): TaskOwner,
    query: Result<Query<AssignmentQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Some(service) = service(&state, &headers) else {
        return task_mode_off();
    };

    // A query that does not parse is a malformed assignment link, said in the
    // task error shape rather than the framework's.
    let Ok(Query(query)) = query else {
        return task_error(AccessError::new("site_invalid"));
    };
    if task_github(&owner.user).is_none() {
        return task_error(AccessError::new("authentication_required"));
    }

    // Said when the task is first shown, not after the PIN, the rules and a
    // calibration have all been spent on an attempt that cannot start.
    if state.dispatcher.is_none() {
        return task_error(AccessError::new("credentials_missing"));
    }
    let assignment = match Assignment::new(&query.site, &set_id, query.version) {
        Ok(assignment) => assignment,
        Err(error) => return task_error(error),
    };
    match service.load_task(owner.user.id, &assignment, &task_id) {
        Ok(mut value) => {
            let languages =
                offered_languages(&value["languages"], state.config.compiler_explorer_enabled);
            if languages.is_empty() {
                return task_error(AccessError::new("language_unavailable"));
            }
            value["languages"] = json!(languages);
            json_response(StatusCode::OK, value)
        }
        Err(error) => task_error(error),
    }
}

/// Whether this server's page can run `language`: C, C++ and Java compile on
/// Compiler Explorer, which a server may turn off.
fn runnable(language: &str, compiler_explorer: bool) -> bool {
    crate::tasks::LANGUAGES.contains(&language)
        && (compiler_explorer || !crate::tasks::COMPILED_LANGUAGES.contains(&language))
}

/// A task's languages this server can run, in the instructor's order, which
/// is the order the page offers them in.
fn offered_languages(languages: &Value, compiler_explorer: bool) -> Vec<&str> {
    languages
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|language| runnable(language, compiler_explorer))
        .collect()
}

/// Bytes of the calibration a page reports with its Start.
const MAX_CALIBRATION_BYTES: usize = 4096;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdmissionRequest {
    site: String,
    version: u32,
    request_key: String,
    language: String,
    supported_platform: bool,
    rules_acknowledged_at: Option<String>,
    calibration: Option<Value>,
}

/// Starts an attempt at one task: the room, the agent and the token the page
/// joins with. Its own route, not a mode of the interview's `/api/token`.
pub(crate) async fn task_start_handler(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path((set_id, task_id)): Path<(String, String)>,
    TaskOwner(owner): TaskOwner,
    request: Request<Body>,
) -> Response {
    let error = |code| task_error(AccessError::new(code));
    let Some(service) = service(&state, request.headers()) else {
        return task_mode_off();
    };
    if !task_csrf(request.headers()) {
        return error("csrf_rejected");
    }
    let client = super::client_ip(request.headers(), peer, state.config.trusted_proxy_hops);
    if !state.token_limit.allow(client, std::time::Instant::now()) {
        return task_error(AccessError {
            code: "admission_throttled",
            retry_after: Some(60),
        });
    }
    let Some((github_login, github_id)) = task_github(&owner.user) else {
        return error("authentication_required");
    };
    let Ok(bytes) = to_bytes(request.into_body(), MAX_BODY_BYTES).await else {
        return task_error(AccessError::new("request_too_large"));
    };
    let Ok(body) = serde_json::from_slice::<AdmissionRequest>(&bytes) else {
        return error("client_override");
    };
    if !runnable(&body.language, state.config.compiler_explorer_enabled) {
        return error("language_unsupported");
    }
    if !body.supported_platform {
        return error("platform_unsupported");
    }
    // Recorded, not proof: the learner's own page reports both.
    let Some(acknowledged) = body
        .rules_acknowledged_at
        .filter(|at| chrono::DateTime::parse_from_rfc3339(at).is_ok())
    else {
        return error("rules_unacknowledged");
    };
    let Some(calibration) = body
        .calibration
        .filter(|value| value.is_object() && value.to_string().len() <= MAX_CALIBRATION_BYTES)
    else {
        return error("calibration_required");
    };
    // A server without the interviewer's credentials has no agent to run.
    let Some(dispatcher) = state.dispatcher.as_ref() else {
        return error("credentials_missing");
    };
    let assignment = match Assignment::new(&body.site, &set_id, body.version) {
        Ok(assignment) => assignment,
        Err(error) => return task_error(error),
    };
    let task = match service.begin_admission(
        owner.user.id,
        (&github_login, github_id),
        &assignment,
        &task_id,
        &body.request_key,
        Preparation {
            rules_acknowledged_at: acknowledged,
            calibration,
            language: body.language,
        },
        crate::current_epoch_seconds(),
    ) {
        Ok(Admission::Replayed(response)) => {
            return json_response(StatusCode::OK, response);
        }
        Ok(Admission::New(task)) => task,
        Err(error) => return task_error(error),
    };
    let claim_id = task.claim_id;
    let duration_min = task.duration_min;
    // Every failure from here on releases the request key it claimed.
    let abandon = || {
        service.finish_admission(owner.user.id, &body.request_key, claim_id, None);
        error("setup_failed")
    };
    let (room_name, provider) = match super::token::reserved_room(&state).await {
        Ok(reserved) => reserved,
        Err(_) => return abandon(),
    };
    let identity = super::token::generated_candidate_identity();
    let metadata = json!({"assessmentMode": "task", "setId": set_id, "setVersion": task.config.version, "taskId": task_id, "candidateIdentity": identity, "taskContractVersion": 1}).to_string();
    let token = match crate::token::livekit_token(crate::token::LivekitTokenInput {
        api_key: &provider.api_key,
        api_secret: &provider.api_secret,
        name: "Learner",
        identity: &identity,
        room: &room_name,
        metadata: &metadata,
        now_seconds: crate::current_epoch_seconds(),
        agent: false,
    }) {
        Ok(token) => token,
        Err(_) => return abandon(),
    };
    if dispatcher
        .ensure_agent(
            &room_name,
            provider,
            super::AgentJob::Task {
                candidate: identity.clone(),
                task,
            },
        )
        .is_err()
    {
        return abandon();
    }
    let response = json!({"token": token, "serverUrl": provider.url, "roomName": room_name, "durationMin": duration_min});
    service.finish_admission(
        owner.user.id,
        &body.request_key,
        claim_id,
        Some(response.clone()),
    );
    json_response(StatusCode::OK, response)
}

#[cfg(test)]
#[path = "../../tests/unit/web/tasks.rs"]
mod tests;

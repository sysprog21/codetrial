//! GitHub sign-in end to end: the OAuth handshake, the signed cookies that
//! carry a session, and the lookups every other module calls to find out who
//! is asking.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, Request, StatusCode, header};
use axum::response::{AppendHeaders, IntoResponse, Response};
use serde_json::{Value, json};

use crate::accounts::{
    Accounts, GitHubLoginConfig, GitHubOauth, GitHubProfile, SESSION_TTL_SECONDS, SignedInUser,
    blocking, create_session, delete_session, normalized_github_login, random_token,
    recorded_account_id, session_user, valid_github_login,
};

use super::{
    AppState, MAX_BODY_BYTES, WebServerConfig, client_ip, json_response,
    oauth_not_configured_response, query_escape, query_pairs, rate_limited_response,
    unauthorized_response,
};

pub(crate) const SESSION_COOKIE: &str = "codetrial_session";
const OAUTH_STATE_COOKIE: &str = "codetrial_oauth_state";
const OAUTH_STATE_TTL_SECONDS: i64 = 60 * 10;

/// `read:user` alone returns whatever address the person made public, which is
/// usually none. Delivery needs the primary verified one, and that is a
/// separate scope and a separate endpoint.
///
/// Asked for only where recording is on. A deployment that records nothing has
/// no use for a candidate's private address, and requesting it anyway would be
/// a consent screen listing an access this server never exercises.
pub(crate) const GITHUB_OAUTH_SCOPE: &str = "read:user";

pub(crate) const GITHUB_OAUTH_SCOPE_WITH_EMAIL: &str = "read:user user:email";

pub(crate) fn github_oauth_scope(recording: bool) -> &'static str {
    if recording {
        GITHUB_OAUTH_SCOPE_WITH_EMAIL
    } else {
        GITHUB_OAUTH_SCOPE
    }
}

pub fn login_config(config: &WebServerConfig) -> Option<GitHubLoginConfig> {
    let (Some(session_secret), Some(db_path)) = (
        config
            .session_secret
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        config
            .db_path
            .as_ref()
            .filter(|value| !value.as_os_str().is_empty()),
    ) else {
        return None;
    };
    let credential = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    Some(GitHubLoginConfig {
        oauth: credential(&config.github_client_id)
            .zip(credential(&config.github_client_secret))
            .map(|(client_id, client_secret)| GitHubOauth {
                client_id,
                client_secret,
            }),
        session_secret: session_secret.to_string(),
        db_path: db_path.clone(),
        oauth_base_url: config
            .github_oauth_base_url
            .clone()
            .unwrap_or_else(|| "https://github.com".to_string()),
        api_base_url: config
            .github_api_base_url
            .clone()
            .unwrap_or_else(|| "https://api.github.com".to_string()),
    })
}

pub(crate) async fn login_handler(State(state): State<AppState>) -> Response {
    let Some(accounts) = state.accounts.clone() else {
        return state.accounts_error();
    };
    let Some(oauth) = &accounts.config().oauth else {
        return oauth_not_configured_response();
    };
    let Ok(state_token) = random_token(24) else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not create login state." }),
        );
    };
    let cookie = set_cookie(
        OAUTH_STATE_COOKIE,
        &signed_cookie_value(&state_token, &accounts.config().session_secret),
        OAUTH_STATE_TTL_SECONDS,
        state.config.production,
    );

    // `user:email` on top of `read:user`, because `/user` returns only a public
    // address and a recording is delivered to a verified one. Escaped rather
    // than written literally: the value now contains a space.
    let location = format!(
        "{}/login/oauth/authorize?client_id={}&state={}&scope={}",
        accounts.config().oauth_base_url.trim_end_matches('/'),
        query_escape(&oauth.client_id),
        query_escape(&state_token),
        query_escape(github_oauth_scope(state.config.recording.is_some()))
    );

    (
        StatusCode::FOUND,
        [(header::LOCATION, location), (header::SET_COOKIE, cookie)],
    )
        .into_response()
}

pub(crate) async fn record_login_handler(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response {
    let Some(accounts) = state.accounts.clone() else {
        return state.accounts_error();
    };
    let client = client_ip(request.headers(), peer, state.config.trusted_proxy_hops);
    if !state.login_limit.allow(client, Instant::now()) {
        return rate_limited_response("Too many login attempts. Wait a minute and try again.");
    }
    let Ok(body) = to_bytes(request.into_body(), MAX_BODY_BYTES).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Ok(body) = serde_json::from_slice::<Value>(&body) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "GitHub login must be JSON." }),
        );
    };
    let Some(login) = body
        .get("login")
        .and_then(Value::as_str)
        .map(normalized_github_login)
        .filter(|login| valid_github_login(login))
    else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Enter a valid GitHub username." }),
        );
    };
    let Ok(github_id) = recorded_account_id() else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not record GitHub login." }),
        );
    };
    let session_secret = accounts.config().session_secret.clone();
    let profile = GitHubProfile {
        github_id,
        login,
        avatar_url: None,

        // A typed handle proves nothing, so it can never carry a delivery
        // address. `create_session` refuses to write one for a negative id too;
        // this is the same rule stated where the value would have come from.
        verified_email: None,
    };
    let session_id = match blocking(move || create_session(&accounts, &profile)).await {
        Ok(session_id) => session_id,
        Err(error) => {
            eprintln!("could not record GitHub login: {error}");
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not record GitHub login." }),
            );
        }
    };

    // Through `json_response` so the no-store policy it documents stays in one
    // place; this route only adds the cookie on top.
    let mut response = json_response(StatusCode::OK, json!({ "ok": true }));
    let cookie = session_cookie(&session_id, &session_secret, state.config.production);
    if let Ok(cookie) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
}

pub(crate) async fn callback_handler(
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    let Some(accounts) = state.accounts.clone() else {
        return state.accounts_error();
    };
    // Without an OAuth app there is no code to exchange.
    let Some(oauth) = accounts.config().oauth.clone() else {
        return oauth_not_configured_response();
    };
    let query = query_pairs(request.uri().query().unwrap_or(""));
    let Some(code) = query.get("code").filter(|value| !value.is_empty()) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Missing GitHub callback code." }),
        );
    };
    let Some(state_token) = query.get("state").filter(|value| !value.is_empty()) else {
        return json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Missing GitHub callback state." }),
        );
    };
    let Some(cookie_state) = cookie_value(request.headers(), OAUTH_STATE_COOKIE)
        .and_then(|value| verified_cookie_value(&value, &accounts.config().session_secret))
    else {
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "Login state is missing or expired." }),
        );
    };
    if cookie_state != *state_token {
        return json_response(
            StatusCode::UNAUTHORIZED,
            json!({ "error": "Login state did not match." }),
        );
    }

    let Ok(access_token) = github_access_token(accounts.config(), &oauth, code).await else {
        return json_response(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "GitHub token exchange failed." }),
        );
    };
    let Ok(profile) = github_profile(
        accounts.config(),
        &access_token,
        state.config.recording.is_some(),
    )
    .await
    else {
        return json_response(
            StatusCode::BAD_GATEWAY,
            json!({ "error": "GitHub profile request failed." }),
        );
    };
    let session_secret = accounts.config().session_secret.clone();
    let Ok(session_id) = blocking(move || create_session(&accounts, &profile)).await else {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not create account session." }),
        );
    };

    (
        StatusCode::FOUND,
        [(header::LOCATION, "/".to_string())],
        AppendHeaders([
            (
                header::SET_COOKIE,
                session_cookie(&session_id, &session_secret, state.config.production),
            ),
            (
                header::SET_COOKIE,
                clear_cookie(OAUTH_STATE_COOKIE, state.config.production),
            ),
        ]),
    )
        .into_response()
}

pub(crate) async fn session_handler(
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    // Never a constant: a server with no cookie secret and no database cannot
    // record a login at all, and telling the page to demand one anyway leaves
    // the candidate typing a username into an endpoint that answers 404.
    let login_required = state.accounts_required;
    // The lobby offers lengths this deployment may not be able to record, and
    // only the server knows the recording cap. Answered on both branches
    // because the duration row is live before anyone signs in.
    let max_duration_min =
        super::token::duration_ceiling(state.config.recording.as_ref().map(|it| it.max_minutes));
    match current_user(state.accounts.as_ref(), request.headers()).await {
        Ok(Some(user)) => json_response(
            StatusCode::OK,
            json!({
                "signedIn": true,
                "loginRequired": login_required,
                "maxDurationMin": max_duration_min,
                "user": {
                    "login": user.login,
                    "avatarUrl": user.avatar_url
                }
            }),
        ),
        Ok(None) => json_response(
            StatusCode::OK,
            json!({
                "signedIn": false,
                "loginRequired": login_required,
                "maxDurationMin": max_duration_min,
            }),
        ),
        Err(_) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Could not read account session." }),
        ),
    }
}

pub(crate) async fn logout_handler(
    State(state): State<AppState>,
    request: Request<Body>,
) -> Response {
    if let Some(accounts) = state.accounts.clone()
        && let Some(session_id) = cookie_value(request.headers(), SESSION_COOKIE)
            .and_then(|value| verified_cookie_value(&value, &accounts.config().session_secret))
    {
        // The cookie is cleared below either way, so a failure here does not
        // change the answer. It does leave a live session row behind until it
        // expires, which is worth a line rather than nothing.
        if let Err(error) = blocking(move || delete_session(&accounts, &session_id)).await {
            eprintln!("could not delete account session on logout: {error}");
        }
    }
    (
        StatusCode::OK,
        [(
            header::SET_COOKIE,
            clear_cookie(SESSION_COOKIE, state.config.production),
        )],
        json!({ "ok": true }).to_string(),
    )
        .into_response()
}

/// The signed-in owner of whatever the route addresses.
///
/// Twelve handlers opened with the same four lines resolving this and
/// returning early. As a parameter the check is part of the signature, so a
/// handler either asks for an owner and runs after one was established, or it
/// has no account to act on. Rejections are the responses those twelve copies
/// already returned.
pub(crate) struct Owner {
    pub(crate) accounts: Arc<Accounts>,
    pub(crate) user: SignedInUser,
}

impl FromRequestParts<AppState> for Owner {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = match current_user(state.accounts.as_ref(), &parts.headers).await {
            Ok(Some(user)) => user,
            Ok(None) => return Err(unauthorized_response()),
            Err(_) => {
                return Err(json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "Could not read account session." }),
                ));
            }
        };
        match state.accounts.clone() {
            Some(accounts) => Ok(Self { accounts, user }),
            None => Err(state.accounts_error()),
        }
    }
}

/// Takes the credentials rather than the whole config, so posting empty ones
/// to GitHub is not something a caller can express.
pub(crate) async fn github_access_token(
    config: &GitHubLoginConfig,
    oauth: &GitHubOauth,
    code: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let response = crate::http_client()
        .post(format!(
            "{}/login/oauth/access_token",
            config.oauth_base_url.trim_end_matches('/')
        ))
        .header(header::ACCEPT, "application/json")
        .json(&json!({
            "client_id": oauth.client_id,
            "client_secret": oauth.client_secret,
            "code": code,
        }))
        .send()
        .await?;
    let body = response.json::<Value>().await?;
    body.get("access_token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "missing GitHub access token".into())
}

pub(crate) async fn github_profile(
    config: &GitHubLoginConfig,
    access_token: &str,
    recording: bool,
) -> Result<GitHubProfile, Box<dyn std::error::Error + Send + Sync>> {
    let body = crate::http_client()
        .get(format!(
            "{}/user",
            config.api_base_url.trim_end_matches('/')
        ))
        .bearer_auth(access_token)
        .header(header::USER_AGENT, "codetrial")
        .send()
        .await?
        .json::<Value>()
        .await?;
    let github_id = body
        .get("id")
        .and_then(Value::as_i64)
        .ok_or("missing GitHub id")?;
    let login = body
        .get("login")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("missing GitHub login")?;
    Ok(GitHubProfile {
        github_id,
        login: login.to_string(),
        avatar_url: body
            .get("avatar_url")
            .and_then(Value::as_str)
            .map(str::to_string),

        // Not fetched at all where recording is off, so a deployment that
        // records nothing never holds a candidate's private address and never
        // fails a sign-in because GitHub rate limited an endpoint it had no
        // reason to call.
        //
        // `None` here also clears an address stored while recording was on,
        // because `create_session` writes what it is given. That is deliberate:
        // a deployment with recording off has no basis to keep a private
        // address, and the cost of turning recording back on is one extra
        // sign-in for anyone who signed in during the gap.
        verified_email: match recording {
            true => primary_verified_email(config, access_token).await?,
            false => None,
        },
    })
}

/// The one address GitHub says is both primary and verified.
///
/// `Ok(None)` means GitHub answered and this account has no such address.
/// `Err` means GitHub did not answer, and the two must not be confused: the
/// caller writes this value into `users.email`, so folding a rate limit or a
/// 502 into "no verified address" would take the delivery address away from
/// somebody who already had one, permanently, on a transient failure of
/// somebody else's server.
///
/// Only the selected address is returned. The response lists every address
/// someone has registered, which is more about them than this pipeline has any
/// business holding.
pub(crate) async fn primary_verified_email(
    config: &GitHubLoginConfig,
    access_token: &str,
) -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let body = crate::http_client()
        .get(format!(
            "{}/user/emails",
            config.api_base_url.trim_end_matches('/')
        ))
        .bearer_auth(access_token)
        .header(header::USER_AGENT, "codetrial")
        .send()
        .await?
        // A 403 from a rate limit still carries a JSON body, and that body
        // parses as a `Value` and is not an array. Without this the failure
        // reaches the `as_array` below and comes back as "no address".
        .error_for_status()?
        .json::<Value>()
        .await?;
    let addresses = body
        .as_array()
        .ok_or("GitHub /user/emails was not a list")?;
    Ok(addresses
        .iter()
        .find(|entry| {
            entry.get("primary").and_then(Value::as_bool) == Some(true)
                && entry.get("verified").and_then(Value::as_bool) == Some(true)
        })
        .and_then(|entry| entry.get("email"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .map(str::to_string))
}

pub(crate) async fn current_user(
    accounts: Option<&Arc<Accounts>>,
    headers: &header::HeaderMap,
) -> rusqlite::Result<Option<SignedInUser>> {
    let Some(accounts) = accounts.cloned() else {
        return Ok(None);
    };
    let Some(session_id) = cookie_value(headers, SESSION_COOKIE)
        .and_then(|value| verified_cookie_value(&value, &accounts.config().session_secret))
    else {
        return Ok(None);
    };
    blocking(move || session_user(&accounts, &session_id)).await
}

pub(crate) fn signed_cookie_value(value: &str, secret: &str) -> String {
    format!("{value}.{}", crate::token::sign_hs256(secret, value))
}

pub(crate) fn verified_cookie_value(value: &str, secret: &str) -> Option<String> {
    let (payload, signature) = value.rsplit_once('.')?;
    crate::token::verify_hs256(secret, payload, signature).then(|| payload.to_string())
}

pub(crate) fn cookie_value(headers: &header::HeaderMap, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|part| part.strip_prefix(&prefix).map(str::to_string))
}

pub(crate) fn set_cookie(name: &str, value: &str, max_age: i64, secure: bool) -> String {
    let secure = if secure { "; Secure" } else { "" };
    format!("{name}={value}; Max-Age={max_age}; Path=/; HttpOnly; SameSite=Lax{secure}")
}

pub(crate) fn clear_cookie(name: &str, secure: bool) -> String {
    set_cookie(name, "", 0, secure)
}

/// Both login paths end the same way, and the signing step is the part that
/// must not be reinvented per handler.
pub(crate) fn session_cookie(session_id: &str, secret: &str, secure: bool) -> String {
    set_cookie(
        SESSION_COOKIE,
        &signed_cookie_value(session_id, secret),
        SESSION_TTL_SECONDS,
        secure,
    )
}

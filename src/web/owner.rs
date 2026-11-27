//! Who is asking: the signed session cookie, the account it names, and the
//! extractor a handler takes to run only for a signed-in owner. Below the
//! sign-in routes in `auth`, which set the cookie; everything that needs an
//! account reads it from here.

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde_json::json;

use crate::accounts::{Accounts, SignedInUser, blocking, session_user};

use super::{AppState, json_response, unauthorized_response};

pub(crate) const SESSION_COOKIE: &str = "codetrial_session";

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
        // Without a database there is no session to read, and that is not a
        // signed-out caller: `accounts_error` says which problem it is.
        let Some(accounts) = state.accounts.clone() else {
            return Err(state.accounts_error());
        };
        let user = match current_user(Some(&accounts), &parts.headers).await {
            Ok(Some(user)) => user,
            Ok(None) => return Err(unauthorized_response()),
            Err(_) => {
                return Err(json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "Could not read account session." }),
                ));
            }
        };
        Ok(Self { accounts, user })
    }
}

pub(crate) async fn current_user(
    accounts: Option<&Arc<Accounts>>,
    headers: &header::HeaderMap,
) -> rusqlite::Result<Option<SignedInUser>> {
    let Some(accounts) = accounts.cloned() else {
        return Ok(None);
    };
    let Some(session_id) =
        verified_cookie(headers, SESSION_COOKIE, &accounts.config().session_secret)
    else {
        return Ok(None);
    };

    // Every caller turns this into a bare 500, so the cause is only recorded
    // here: a locked or corrupt database otherwise fails all signed-in traffic
    // with nothing for the operator to read.
    blocking(move || session_user(&accounts, &session_id))
        .await
        .inspect_err(|error| eprintln!("account session lookup failed: {error}"))
}

pub(crate) fn verified_cookie_value(name: &str, value: &str, secret: &str) -> Option<String> {
    let (payload, signature) = value.rsplit_once('.')?;
    crate::token::verify_hs256(secret, &format!("{name}={payload}"), signature)
        .then(|| payload.to_string())
}

/// The payload of cookie `name`, if it is present and signed for that name.
pub(crate) fn verified_cookie(
    headers: &header::HeaderMap,
    name: &str,
    secret: &str,
) -> Option<String> {
    verified_cookie_value(name, &cookie_value(headers, name)?, secret)
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

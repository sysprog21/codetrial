//! What a request is held to before anything acts on it: how long and how much
//! of its body is read, how often one client may ask, and how long an
//! interview may be. Shared by the routes that mint a token, sign someone in
//! and take a task or replay upload, so none of them owns it.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes, to_bytes};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::config::{MAX_DURATION_MIN, MIN_DURATION_MIN};

use super::json_response;

pub(crate) const TOKEN_RATE_WINDOW: Duration = Duration::from_secs(60);

/// A partial upload must not hold a token request open forever. This deadline
/// ends before any room is reserved, consent is claimed, or agent is started.
pub(crate) const TOKEN_BODY_READ_LIMIT: Duration = Duration::from_secs(10);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TokenBodyReadError {
    TimedOut,
    Rejected,
}

/// The body of a request nothing has authenticated yet, or a refusal to send
/// as the answer: a stalled client is told it timed out and an oversized one
/// that it sent too much, so neither holds the connection open.
#[expect(
    clippy::result_large_err,
    reason = "the error is the response sent to the client"
)]
pub(crate) async fn read_body_or_refuse(body: Body, max_bytes: usize) -> Result<Bytes, Response> {
    read_body_with(body, max_bytes, || {
        StatusCode::PAYLOAD_TOO_LARGE.into_response()
    })
    .await
}

/// The same read for a route that answers an oversized body in a shape of its
/// own: the stalled client still gets the timeout, so no route holds a
/// connection open for a body that is never finished.
#[expect(
    clippy::result_large_err,
    reason = "the error is the response sent to the client"
)]
pub(crate) async fn read_body_with(
    body: Body,
    max_bytes: usize,
    too_large: impl FnOnce() -> Response,
) -> Result<Bytes, Response> {
    read_token_body(body, TOKEN_BODY_READ_LIMIT, max_bytes)
        .await
        .map_err(|error| match error {
            TokenBodyReadError::TimedOut => json_response(
                StatusCode::REQUEST_TIMEOUT,
                json!({ "error": "Request body timed out." }),
            ),
            TokenBodyReadError::Rejected => too_large(),
        })
}

pub(crate) async fn read_token_body(
    body: Body,
    limit: Duration,
    max_bytes: usize,
) -> Result<Bytes, TokenBodyReadError> {
    match tokio::time::timeout(limit, to_bytes(body, max_bytes)).await {
        Err(_) => Err(TokenBodyReadError::TimedOut),
        Ok(Err(_)) => Err(TokenBodyReadError::Rejected),
        Ok(Ok(body)) => Ok(body),
    }
}

/// The key is what the route can name a caller by. `/api/token` and
/// `/api/login` are reachable without a credential, so they key on the address;
/// replay ingestion runs behind `Owner` and keys on the account. The window is
/// shared because all three buckets want the same minute.
#[derive(Clone)]
pub(crate) struct TokenRateLimit<K = IpAddr> {
    state: Arc<Mutex<RateLimitState<K>>>,
    limit: u32,
}

struct RateLimitState<K> {
    windows: HashMap<K, Window>,
    last_sweep: Instant,
}

impl<K> Default for RateLimitState<K> {
    fn default() -> Self {
        Self {
            windows: HashMap::new(),
            last_sweep: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Window {
    started: Instant,
    count: u32,
}

impl<K: Eq + std::hash::Hash> TokenRateLimit<K> {
    pub(crate) fn with_limit(limit: u32) -> Self {
        Self {
            state: Arc::default(),
            limit,
        }
    }

    /// Crosses a module boundary: the token handlers, the login handler in
    /// `super::auth` and the replay handler in `super::interviews` each hold
    /// their own limiter of this type.
    pub(crate) fn allow(&self, client: K, now: Instant) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());

        // Sweeping every tracked client on every call cost O(clients) per
        // request while holding the lock, so a flood from many addresses was
        // absorbed by the limiter into more work per request: the control
        // amplifying what it exists to damp. The entry being touched expires
        // itself below, and the sweep that keeps the map bounded now runs at
        // most once per window.
        if now.duration_since(state.last_sweep) >= TOKEN_RATE_WINDOW {
            state
                .windows
                .retain(|_, window| now.duration_since(window.started) < TOKEN_RATE_WINDOW);
            state.last_sweep = now;
        }
        let window = state.windows.entry(client).or_insert(Window {
            started: now,
            count: 0,
        });
        if now.duration_since(window.started) >= TOKEN_RATE_WINDOW {
            *window = Window {
                started: now,
                count: 0,
            };
        }
        window.count += 1;
        window.count <= self.limit
    }
}

/// The longest interview this deployment will start, which is its recording's
/// bound wherever it records: the sweeper reaps a recording that outlives
/// `max_minutes`, so a longer interview loses the stretch past it.
///
/// One function because two callers need the same answer and must not drift.
/// `/api/token` enforces it, and `/api/session` hands it to the lobby so the
/// browser stops offering what this server would silently shorten.
///
/// Clamped into the range rather than taken as given: a cap above
/// `MAX_DURATION_MIN` does not buy a longer interview than the endpoint has
/// ever offered, and one below `MIN_DURATION_MIN` would cross the floor, which
/// is a panic in `f64::clamp` rather than a short interview.
pub(crate) fn duration_ceiling(recording_max_min: Option<u32>) -> u32 {
    recording_max_min.map_or(MAX_DURATION_MIN, |cap| {
        cap.clamp(MIN_DURATION_MIN, MAX_DURATION_MIN)
    })
}

/// The operator's `CODETRIAL_DURATION_MIN` as the lobby and `/api/token` both
/// use it: inside the range the endpoint offers and no longer than the
/// deployment can record. One function for the same reason as
/// [`duration_ceiling`].
pub(crate) fn default_duration(configured_min: u32, recording_max_min: Option<u32>) -> u32 {
    configured_min
        .clamp(MIN_DURATION_MIN, MAX_DURATION_MIN)
        .min(duration_ceiling(recording_max_min))
}

#[cfg(test)]
#[path = "../../tests/unit/web/limits.rs"]
mod tests;

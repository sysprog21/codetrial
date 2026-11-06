use crate::web::{AppState, Owner, json_response};
use axum::extract::FromRequestParts;
use axum::http::{Method, StatusCode, header, request::Parts};
use axum::response::Response;
use serde_json::json;

pub(super) struct VerifiedPeer(pub Owner);
impl FromRequestParts<AppState> for VerifiedPeer {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let owner = Owner::from_request_parts(parts, state).await?;
        if !owner.user.identity_verified {
            return Err(json_response(
                StatusCode::FORBIDDEN,
                json!({"code":"verified_identity_required"}),
            ));
        }
        if !matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS) {
            let origin = parts
                .headers
                .get(header::ORIGIN)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| reqwest::Url::parse(v).ok());
            let host = parts
                .headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
                .or_else(|| parts.uri.authority().map(|authority| authority.as_str()));
            let matches = origin.as_ref().is_some_and(|o| {
                let expected = host
                    .and_then(|host| reqwest::Url::parse(&format!("{}://{host}", o.scheme())).ok());
                o.username().is_empty()
                    && o.password().is_none()
                    && o.path() == "/"
                    && o.query().is_none()
                    && o.fragment().is_none()
                    && ["http", "https"].contains(&o.scheme())
                    && (!state.config.production || o.scheme() == "https")
                    && expected.is_some_and(|expected| {
                        expected.username().is_empty()
                            && expected.password().is_none()
                            && expected.origin() == o.origin()
                    })
            });
            if !matches {
                return Err(json_response(
                    StatusCode::FORBIDDEN,
                    json!({"code":"origin_mismatch"}),
                ));
            }
        }
        let snapshot = parts.method == Method::PUT && is_snapshot_path(parts.uri.path());
        let bucket = if snapshot {
            &state.peer_snapshot_limit
        } else {
            &state.peer_limit
        };
        if !bucket.allow(owner.user.id, std::time::Instant::now()) {
            use axum::response::IntoResponse;
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, "60")],
                axum::Json(json!({"code":"rate_limited"})),
            )
                .into_response());
        }
        Ok(Self(owner))
    }
}

/// The handler routes on the percent-decoded path, so the limiter must too:
/// `%73napshot` reaches the snapshot route and belongs in its bucket.
fn is_snapshot_path(raw: &str) -> bool {
    let mut rest = raw.as_bytes();
    let mut decoded = Vec::with_capacity(rest.len());
    while let Some((&first, tail)) = rest.split_first() {
        let escaped = (first == b'%')
            .then(|| tail.get(..2))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(byte) = escaped {
            decoded.push(byte);
            rest = &tail[2..];
        } else {
            decoded.push(first);
            rest = tail;
        }
    }
    decoded.ends_with(b"/snapshot")
}

#[cfg(test)]
#[path = "../../../tests/unit/web/peer/auth.rs"]
mod tests;

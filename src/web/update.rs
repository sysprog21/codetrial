//! The update notice: whether the `latest` release names a different commit
//! from the one this binary was built from.
//!
//! The server asks GitHub and the lobby asks the server, so the page's
//! `connect-src` gains nothing and every open tab shares one answer. Nothing is
//! asked at all unless `CODETRIAL_UPDATE_CHECK` turned it on for a macOS build
//! CI made (`config::update_check_commit`).
//!
//! Every failure answers "no update": no network, a GitHub error, a rate limit,
//! and the window in which `latest` resolves to nothing while it is republished
//! all leave the lobby as it was, because a notice that only says the check
//! failed is not something a candidate can act on.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};

use super::{AppState, json_response};

/// Where the notice sends the candidate: the release page rather than the
/// asset, so the notes naming the build sit beside the download.
pub(crate) const RELEASE_PAGE: &str = "https://github.com/sysprog21/codetrial/releases/tag/latest";

const RELEASE_PATH: &str = "/repos/sysprog21/codetrial/releases/tags/latest";
const DEFAULT_API_BASE: &str = "https://api.github.com";

/// How long one answer serves every tab. The lobby asks when it loads and on
/// the hour, so this is what bounds reloads and a row of open tabs: at most six
/// requests an hour, inside the 60 GitHub allows an address that does not sign
/// in.
const ANSWER_TTL: Duration = Duration::from_mins(10);

/// The lobby is waiting on this, so it gets far less than the client's 30
/// second backstop.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// When GitHub was asked, and the full commit it named, or `None` when it
/// could not say.
type Asked = (Instant, Option<String>);

/// The commit GitHub last named as `latest`, and when it was asked.
#[derive(Clone, Default)]
pub(crate) struct LatestRelease {
    answer: Arc<tokio::sync::Mutex<Option<Asked>>>,
}

impl LatestRelease {
    /// The full commit `latest` names, or `None` when GitHub could not say.
    ///
    /// A failure is kept as long as an answer, so a machine with no network
    /// does not ask again on every reload. The lock is held across the request
    /// so that tabs asking at the same moment share it rather than each making
    /// their own.
    async fn commit(&self, api_base: &str) -> Option<String> {
        self.commit_at(api_base, Instant::now()).await
    }

    /// [`Self::commit`] at a given moment, so a test can age the answer by
    /// moving forward: on a container that started minutes ago, an `Instant`
    /// ten minutes in the past does not exist.
    ///
    /// Spawned rather than awaited in place: a lobby that reloads or closes
    /// mid-request drops this future, which would cancel the request and keep
    /// nothing, so the next load would ask GitHub again however often that
    /// happened.
    async fn commit_at(&self, api_base: &str, now: Instant) -> Option<String> {
        let answer = Arc::clone(&self.answer);
        let api_base = api_base.to_string();
        tokio::spawn(async move {
            let mut answer = answer.lock().await;
            if let Some((asked, commit)) = answer.as_ref()
                && now.saturating_duration_since(*asked) < ANSWER_TTL
            {
                return commit.clone();
            }
            let commit = fetch_latest_commit(&api_base).await;
            *answer = Some((now, commit.clone()));
            commit
        })
        .await
        .ok()
        .flatten()
    }
}

/// `target_commitish` rather than the tag's ref: the release job publishes
/// with `--target "$GITHUB_SHA"`, so it is the commit the assets were built
/// from, and one read answers the question. Anything but a full SHA is refused,
/// since a branch name there would compare as an update to every build.
async fn fetch_latest_commit(api_base: &str) -> Option<String> {
    let request = crate::no_redirect_http_client()
        .get(format!("{}{RELEASE_PATH}", api_base.trim_end_matches('/')))
        .timeout(CHECK_TIMEOUT)
        .header(header::USER_AGENT, "codetrial")
        .header(header::ACCEPT, "application/vnd.github+json");
    let release = super::auth::github_json(request)
        .await
        .inspect_err(|error| eprintln!("update check: could not read the latest release: {error}"))
        .ok()?;
    release
        .get("target_commitish")
        .and_then(Value::as_str)
        .filter(|commit| commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
}

pub(crate) async fn update_handler(State(state): State<AppState>) -> Response {
    let Some(current) = state.config.update_check_commit.as_deref() else {
        return json_response(StatusCode::OK, json!({ "enabled": false }));
    };
    let api_base = state
        .config
        .github_api_base_url
        .as_deref()
        .unwrap_or(DEFAULT_API_BASE);
    let latest = state.latest_release.commit(api_base).await;
    json_response(StatusCode::OK, answer(current, latest.as_deref()))
}

/// `current` is the seven-character commit the build recorded, `latest` the
/// full one GitHub named.
fn answer(current: &str, latest: Option<&str>) -> Value {
    match latest {
        Some(latest) if !latest.starts_with(current) => json!({
            "enabled": true,
            "available": true,
            "current": current,
            "latest": &latest[..7],
            "url": RELEASE_PAGE,
        }),
        _ => json!({ "enabled": true, "available": false }),
    }
}

#[cfg(test)]
#[path = "../../tests/unit/web/update.rs"]
mod tests;

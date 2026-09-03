//! The solo self-serve cold start's Setup page: served instead of the full
//! app when `run_web` finds no config file at all.

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{Html, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::config::load_from_pairs;
use crate::gemini::{gemini_live_websocket_url, open_live_session_at};
use crate::runtime::bootstrap;

/// Notified on a successful submission: the caller's shutdown signal to
/// drop this listener and rebind as the full app.
pub fn setup_service(ready: Arc<Notify>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/", get(setup_page))
        .route(
            "/api/setup",
            post(move |body| submit_setup(body, ready.clone())),
        )
}

async fn setup_page() -> Html<&'static str> {
    Html(SETUP_PAGE)
}

/// Field `name`s are the contract with `submit_setup`'s JSON keys. Inline,
/// not a `web/` file: served before any config exists, outside the embedded
/// asset tree.
const SETUP_PAGE: &str = r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>codetrial Setup</title>
<style>
  body { font-family: system-ui, sans-serif; max-width: 32rem; margin: 2rem auto; padding: 0 1rem; }
  label { display: block; margin-top: 1rem; font-weight: 600; }
  input { display: block; width: 100%; margin-top: .25rem; padding: .4rem; box-sizing: border-box; }
  small { display: block; color: #555; margin-top: .15rem; }
  button { margin-top: 1.5rem; padding: .5rem 1rem; }
  #status { margin-top: 1rem; white-space: pre-wrap; }
  #status.error { color: #b00020; }
  #status.ok { color: #0a7a2f; }
</style>
</head>
<body>
<h1>Setup</h1>
<p>Enter your LiveKit and Google credentials to get started.</p>
<form id="setup-form">
  <label for="livekitUrl">LiveKit URL</label>
  <input id="livekitUrl" name="livekitUrl" type="text" placeholder="wss://your-project.livekit.cloud" required>
  <small>From your project at <a href="https://cloud.livekit.io" target="_blank" rel="noopener">cloud.livekit.io</a>.</small>

  <label for="livekitApiKey">LiveKit API Key</label>
  <input id="livekitApiKey" name="livekitApiKey" type="text" required>

  <label for="livekitApiSecret">LiveKit API Secret</label>
  <input id="livekitApiSecret" name="livekitApiSecret" type="password" required>
  <small>Both come from the same project's Keys page.</small>

  <label for="googleApiKey">Google API Key</label>
  <input id="googleApiKey" name="googleApiKey" type="password">
  <small>From <a href="https://aistudio.google.com/apikey" target="_blank" rel="noopener">aistudio.google.com</a>.</small>

  <button type="submit">Save and continue</button>
</form>
<div id="status"></div>
<script>
  const form = document.getElementById('setup-form');
  const status = document.getElementById('status');

  async function waitForRestart() {
    // Server rebinds after success; poll instead of reloading once so the gap is invisible.
    for (let attempt = 0; attempt < 40; attempt++) {
      await new Promise((resolve) => setTimeout(resolve, 250));
      try {
        const probe = await fetch('/healthz', { cache: 'no-store' });
        if (probe.ok) break;
      } catch (error) {
        // keep polling
      }
    }
    location.reload();
  }

  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    status.className = '';
    status.textContent = 'Checking credentials…';
    const data = Object.fromEntries(new FormData(form).entries());
    let response;
    try {
      response = await fetch('/api/setup', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(data),
      });
    } catch (error) {
      status.className = 'error';
      status.textContent = 'Request failed: ' + error;
      return;
    }
    const body = await response.json().catch(() => ({}));
    if (response.ok) {
      status.className = 'ok';
      status.textContent = 'Saved. Starting codetrial…';
      waitForRestart();
    } else {
      status.className = 'error';
      status.textContent = body.error || ('Request failed: ' + response.status);
    }
  });
</script>
</body>
</html>
"#;

/// Writes `codetrial.env.local` into the executable's own folder, which is
/// the last path `primary_config_path` searches: the file has to be found
/// again on the next launch, and that folder is the one thing about a
/// double-clicked binary that does not depend on where it was started from.
/// Parsed as `Value`, not a derived struct — this crate has no `serde`
/// derive dependency, and a missing field reads as empty rather than a
/// parse error.
async fn submit_setup(Json(submission): Json<Value>, ready: Arc<Notify>) -> Response {
    let field = |key: &str| {
        submission
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };

    // Field order, so the first missing one is reported. `googleApiKey` is
    // optional here, same as an operator's config file: empty means web-only,
    // no interviewer hosted by this process.
    for key in ["livekitUrl", "livekitApiKey", "livekitApiSecret"] {
        if field(key).is_empty() {
            return super::json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("{key} is required") }),
            );
        }
    }

    let livekit_url = field("livekitUrl");
    let livekit_api_key = field("livekitApiKey");
    let livekit_api_secret = field("livekitApiSecret");
    let google_api_key = field("googleApiKey");

    if let Err(error) = crate::livekit::validate_livekit_credentials(
        &livekit_url,
        &livekit_api_key,
        &livekit_api_secret,
        crate::current_epoch_seconds(),
    )
    .await
    {
        return super::json_response(
            StatusCode::BAD_REQUEST,
            json!({
                "error": format!(
                    "livekitUrl, livekitApiKey or livekitApiSecret did not work: {error}"
                )
            }),
        );
    }

    if !google_api_key.is_empty() {
        let pairs = [
            ("LIVEKIT_URL", livekit_url.clone()),
            ("LIVEKIT_API_KEY", livekit_api_key.clone()),
            ("LIVEKIT_API_SECRET", livekit_api_secret.clone()),
            ("GOOGLE_API_KEY", google_api_key.clone()),
        ];
        let config = match load_from_pairs(pairs) {
            Ok(config) => config,
            Err(error) => {
                return super::json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": error.to_string() }),
                );
            }
        };

        // Same proof as `check-gemini`: open a real session. `CODETRIAL_GEMINI_LIVE_URL`
        // lets tests redirect this away from the real endpoint.
        let room_name = format!("{}-smoke", config.room_prefix);
        let boot = bootstrap(&config, &room_name, None, config.default_duration_min);
        let url = std::env::var("CODETRIAL_GEMINI_LIVE_URL")
            .unwrap_or_else(|_| gemini_live_websocket_url(&config.google_api_key));
        match open_live_session_at(&url, &boot, None).await {
            Ok(session) => {
                let _ = session.close().await;
            }
            Err(error) => {
                return super::json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": format!("googleApiKey did not work: {error}") }),
                );
            }
        }
    }

    let contents = format!(
        "LIVEKIT_URL={livekit_url}\nLIVEKIT_API_KEY={livekit_api_key}\nLIVEKIT_API_SECRET={livekit_api_secret}\nGOOGLE_API_KEY={google_api_key}\n",
    );
    let path = crate::exe_dir().join("codetrial.env.local");
    if let Err(error) = std::fs::write(&path, contents) {
        return super::json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("could not write {}: {error}", path.display()) }),
        );
    }
    // Only reached once the file is on disk; tells the caller to rebind as the full app.
    ready.notify_one();
    super::json_response(StatusCode::OK, json!({ "saved": true }))
}

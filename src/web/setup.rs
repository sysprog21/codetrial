//! The solo self-serve cold start's Setup page: served instead of the full
//! app when `run_web` finds no config file at all.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::config::load_from_pairs;
use crate::gemini::{gemini_live_websocket_url, open_live_session_at};
use crate::runtime::bootstrap;

/// Notified on a successful submission: the caller's shutdown signal to stop
/// serving Setup and continue as the full app on the same socket.
///
/// `production` is the caller's, not one read here. The launch this hands over
/// to enforces the production rules on the file this writes, and a second
/// reading of `NODE_ENV` is a second answer waiting to disagree with it.
///
/// `port` is the one the listener bound, which is what a `Host` has to name.
/// `config_path` is where a submission is written, decided by the caller: the
/// rest of the path rules live there and a second answer here could disagree
/// with the search the next launch performs.
pub fn setup_service(
    ready: Arc<Notify>,
    production: bool,
    port: u16,
    config_path: PathBuf,
) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/", get(setup_page))
        .route(
            "/api/setup",
            post(move |body| submit_setup(body, ready.clone(), production, config_path.clone())),
        )
        .layer(middleware::from_fn(
            move |request: Request, next: Next| async move {
                if host_is_loopback(request.headers().get(header::HOST), port) {
                    next.run(request).await
                } else {
                    super::json_response(
                        StatusCode::FORBIDDEN,
                        json!({
                            "error": "Setup answers only to a loopback Host. Open it at the \
                                      address the console printed."
                        }),
                    )
                }
            },
        ))
}

/// The `Host` a browser sends for a loopback origin on `port`, and nothing
/// else.
///
/// The loopback bind keeps the network out, not a browser. Any origin can give
/// a name a short TTL, repoint it at 127.0.0.1 after the first load and POST
/// here as same origin, which skips the preflight `Content-Type:
/// application/json` would otherwise force. That submission writes the
/// attacker's own LiveKit project into the config every later interview routes
/// through, and the rebound name is the one thing the request still carries.
fn host_is_loopback(host: Option<&HeaderValue>, port: u16) -> bool {
    let Some(host) = host.and_then(|value| value.to_str().ok()) else {
        return false;
    };
    // An IPv6 literal is bracketed, so a colon inside the brackets is not the
    // port separator.
    let (name, host_port) = match host.rsplit_once(':') {
        Some((name, digits)) if !digits.contains(']') => (name, Some(digits)),
        _ => (host, None),
    };
    let port_matches = match host_port {
        Some(digits) => digits.parse::<u16>() == Ok(port),
        // A browser omits the port only for the scheme's default, and this page
        // is plain HTTP.
        None => port == 80,
    };
    port_matches
        && matches!(
            name.to_ascii_lowercase().as_str(),
            "localhost" | "127.0.0.1" | "[::1]"
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

  <button id="submit" type="submit">Save and continue</button>
</form>
<div id="status"></div>
<script>
  const form = document.getElementById('setup-form');
  const status = document.getElementById('status');
  const submit = document.getElementById('submit');

  async function waitForRestart() {
    // The server keeps the port and swaps what answers on it, so a request landing in
    // the handover waits in the accept backlog rather than failing. Poll anyway: the
    // gap is not zero. /api/session and not /healthz, because this page's own server
    // answers /healthz too and a probe that beats its shutdown reloads into the gap.
    for (let attempt = 0; attempt < 40; attempt++) {
      await new Promise((resolve) => setTimeout(resolve, 250));
      try {
        const probe = await fetch('/api/session', { cache: 'no-store' });
        if (probe.ok) break;
      } catch (error) {
        // keep polling
      }
    }
    location.reload();
  }

  form.addEventListener('submit', async (event) => {
    event.preventDefault();
    // A second submission would reach a config file the first one has already
    // written, and the write refuses to overwrite: the double-click would report
    // a failure over a setup that worked. Re-enabled only where the form is
    // still the way forward, which a successful submission is not.
    submit.disabled = true;
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
      submit.disabled = false;
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
      submit.disabled = false;
    }
  });
</script>
</body>
</html>
"#;

/// Writes the config to `config_path`, which `primary_config_path` searches
/// for: the file has to be found again on the next launch, from whatever
/// directory that launch happens to start in.
///
/// Parsed as `Value`, not a derived struct — this crate has no `serde`
/// derive dependency, and a missing field reads as empty rather than a
/// parse error.
async fn submit_setup(
    Json(submission): Json<Value>,
    ready: Arc<Notify>,
    production: bool,
    path: PathBuf,
) -> Response {
    // Trimmed here, once, because everything downstream assumes it was. A
    // pasted URL keeps its leading space through `validate_livekit_url`, which
    // trims to decide and hands the original on, and `livekit_scheme` then
    // matches at offset 0 and rewrites nothing: working credentials come back
    // as "did not work". A key with a trailing space is signed into the JWT
    // verbatim and refused, though the same value hand-written into the config
    // file works, because `read_config_file` trims what it reads. And a
    // `googleApiKey` of spaces is not empty to `is_empty`, so it takes the
    // branch that probes Gemini and fails there for a field the form calls
    // optional.
    let field = |key: &str| {
        submission
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };

    // Field order, so the first missing one is reported. `googleApiKey` is
    // optional here, same as an operator's config file: empty means web-only,
    // no interviewer hosted by this process.
    for key in ["livekitUrl", "livekitApiKey", "livekitApiSecret"] {
        // A value of spaces is empty by now, which is what it has to be:
        // `read_config_file` trims when it reads this back and `nonempty` then
        // calls it missing, so accepting one here would write a file the launch
        // on the other side of this page refuses.
        if field(key).is_empty() {
            return super::json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("{key} is required") }),
            );
        }
    }

    // Every field, including the optional one, and before anything is sent
    // anywhere. These are written as `KEY=value` lines below and read back by
    // `read_config_file`, which parses one line at a time: a newline inside a
    // value is a second line, and a second line is a config key nobody
    // submitted. `SESSION_SECRET` is one such key, and this process reads the
    // file it just wrote.
    for key in [
        "livekitUrl",
        "livekitApiKey",
        "livekitApiSecret",
        "googleApiKey",
    ] {
        if field(key).chars().any(char::is_control) {
            return super::json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": format!("{key} must not contain control characters") }),
            );
        }
    }

    let livekit_url = field("livekitUrl");
    let livekit_api_key = field("livekitApiKey");
    let livekit_api_secret = field("livekitApiSecret");
    let google_api_key = field("googleApiKey");

    // The keys this submission becomes, in the order the file below writes
    // them. One list: what the launch is asked about has to be what gets
    // written, or the answer was about a different config.
    //
    // A blank `googleApiKey` leaves the key out rather than writing it empty.
    // `read_config_file` keeps an empty value, and `load_values` lays file
    // pairs over the environment, so the line would erase a `GOOGLE_API_KEY`
    // the operator had exported and drop the process to web-only -- announced
    // on a console a double-clicked binary does not have. A hand-written config
    // omits the key to defer to the environment, and this writes the same file.
    let mut pairs = vec![
        ("LIVEKIT_URL", livekit_url.as_str()),
        ("LIVEKIT_API_KEY", livekit_api_key.as_str()),
        ("LIVEKIT_API_SECRET", livekit_api_secret.as_str()),
    ];
    if !google_api_key.is_empty() {
        pairs.push(("GOOGLE_API_KEY", google_api_key.as_str()));
    }

    // The rule the launch on the other side of this page applies to the URL,
    // applied while there is still a form to report it in. Without it a URL
    // this accepts and `web_provider_pool` refuses is written, answered with
    // 200, and then kills the process that was about to serve it -- and because
    // the file now exists, the next launch is no longer a cold start, so the
    // page never comes back to correct it. Before the probes, which are the
    // slow half and reach the network.
    //
    // The URL is one rule of several that `run_web` applies to this file; the
    // rest still fire after the write, which is a separate change.
    //
    // Reported as it stands, naming `LIVEKIT_URL` rather than the form's
    // `livekitUrl`: the two spellings are the same value, and the one in the
    // message is what the reader will find in the file afterwards.
    if let Err(error) = crate::config::validate_livekit_url(&livekit_url, production) {
        return super::json_response(StatusCode::BAD_REQUEST, json!({ "error": error }));
    }

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
        let config = match load_from_pairs(pairs.iter().copied()) {
            Ok(config) => config,
            Err(error) => {
                return super::json_response(
                    StatusCode::BAD_REQUEST,
                    json!({ "error": error.to_string() }),
                );
            }
        };

        // Same proof as `check-gemini`: open a real session.
        // `CODETRIAL_GEMINI_LIVE_URL` lets tests redirect this away from the
        // real endpoint.
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

    // From `pairs`, so the file holds exactly the config that was checked and
    // probed above. Every value is known to carry no newline by now, which is
    // what makes one line per key a faithful encoding of it.
    let contents = pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect::<String>();
    // The directory is the caller's answer, and a released binary's copy of it
    // does not exist until the first submission.
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return super::json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("could not create {}: {error}", parent.display()) }),
        );
    }
    // `create_new`, so an existing name is reported rather than followed and
    // truncated. `is_cold_start` reached this page by finding no config, and
    // the `is_file` it asked answers no for a symlink with nothing at the end
    // of it: without `O_EXCL` a dangling one planted here sends these
    // credentials wherever it points.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        // Otherwise the umask decides, and its usual answer is world-readable.
        // This file holds `LIVEKIT_API_SECRET` and `GOOGLE_API_KEY`. Windows
        // has no equivalent here and inherits the folder's ACL.
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, contents.as_bytes()));
    if let Err(error) = written {
        let reason = if error.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "{} already exists; move it aside and reload",
                path.display()
            )
        } else {
            format!("could not write {}: {error}", path.display())
        };
        return super::json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": reason }),
        );
    }

    // Only reached once the file is on disk; tells the caller to stop serving
    // Setup and continue as the full app.
    ready.notify_one();
    super::json_response(StatusCode::OK, json!({ "saved": true }))
}

#[cfg(test)]
mod tests {
    use super::host_is_loopback;
    use axum::http::HeaderValue;

    fn host(value: &str) -> HeaderValue {
        HeaderValue::from_str(value).expect("a test Host should be a header value")
    }

    #[test]
    fn a_loopback_host_naming_the_bound_port_is_accepted() {
        for value in [
            "127.0.0.1:3000",
            "localhost:3000",
            "[::1]:3000",
            "LocalHost:3000",
        ] {
            assert!(host_is_loopback(Some(&host(value)), 3000), "{value}");
        }
    }

    /// The rebinding case: the name is the attacker's, the port is ours, and
    /// the port is the half that matches.
    #[test]
    fn a_host_that_is_not_a_loopback_name_is_refused() {
        for value in [
            "codetrial.example:3000",
            "127.0.0.1.codetrial.example:3000",
            "localhost.codetrial.example:3000",
        ] {
            assert!(!host_is_loopback(Some(&host(value)), 3000), "{value}");
        }
    }

    /// A `Host` is only missing or portless from something that is not the
    /// browser this page was opened in, since the console printed the port.
    #[test]
    fn a_missing_or_mismatched_port_is_refused() {
        assert!(!host_is_loopback(None, 3000));
        assert!(!host_is_loopback(Some(&host("127.0.0.1")), 3000));
        assert!(!host_is_loopback(Some(&host("127.0.0.1:3001")), 3000));
        assert!(!host_is_loopback(Some(&host("[::1]")), 3000));
        assert!(!host_is_loopback(Some(&host("127.0.0.1:")), 3000));
    }

    /// Port 80 is the one a browser leaves out of the `Host`, so it is the one
    /// case where a portless value is the real thing rather than a hand-written
    /// request.
    #[test]
    fn the_default_http_port_is_accepted_without_one() {
        assert!(host_is_loopback(Some(&host("localhost")), 80));
        assert!(host_is_loopback(Some(&host("localhost:80")), 80));
        assert!(!host_is_loopback(Some(&host("localhost")), 3000));
    }
}

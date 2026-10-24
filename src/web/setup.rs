//! The solo self-serve cold start's Setup page: served instead of the full
//! app when `run_web` finds no config file at all.

use std::path::{Path, PathBuf};
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
///
/// `offer` is a config a launcher found, such as the CLI's, whose keys the
/// page offers to use. Only a summary of it reaches the page, the LiveKit host
/// and whether a Google key is there: any account on the machine can reach this
/// loopback port while Setup serves, and the file's own permissions may keep
/// some of them out of it. Using it is a submission the server reads the file
/// for.
///
/// No `/healthz`. Nothing polls it here any more, and the full app routes it,
/// so its absence is the readiness answer a probe wants: a container
/// healthcheck against a process that has no config and is serving a
/// credential form used to be told 200 by a server that will never host an
/// interview.
pub fn setup_service(
    ready: Arc<Notify>,
    production: bool,
    port: u16,
    config_path: PathBuf,
    offer: Option<PathBuf>,
) -> Router {
    let summarized = offer.clone();
    Router::new()
        .route("/", get(setup_page))
        .route(
            "/api/setup/offer",
            get(move || async move { offer_summary(summarized.as_deref()) }),
        )
        .route(
            "/api/setup",
            post(move |body| {
                submit_setup(
                    body,
                    ready.clone(),
                    production,
                    config_path.clone(),
                    offer.clone(),
                )
            }),
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
///
/// The Google field says what leaving it blank buys, because the only other
/// place that says so is `run_web`'s `eprintln!`, and this page exists for a
/// launch with no console to print it on. Blank is a server that looks
/// finished: the lobby loads, the interview starts, and nothing ever joins.
const SETUP_PAGE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>CodeTrial - Setup</title>
<style>
  :root {
    color-scheme: light;
    --page: #f6f6f9;
    --surface: #ffffff;
    --ink: #171d2d;
    --sub: #4f5b73;
    --hairline: #e3e5ec;
    --control-border: #858fa3;
    --accent: #6e2bf0;
    --accent-fill: #6e2bf0;
    --accent-deep: #5520c4;
    --accent-ink: #5520c4;
    --practice-tint: #f3edff;
    --practice-tint-edge: #ddd0fb;
    --good: #137b56;
    --critical: #c52d43;
    --shadow-card: 0 1px 2px rgb(23 29 45 / 4%), 0 8px 24px rgb(23 29 45 / 6%);
  }
  @media (prefers-color-scheme: dark) {
    :root {
      color-scheme: dark;
      --page: #11110f;
      --surface: #1b1b18;
      --ink: #f6f3ea;
      --sub: #b7b2a5;
      --hairline: #2d2b26;
      --control-border: #8a8473;
      --accent: #a291ff;
      --accent-fill: #744ce6;
      --accent-deep: #6437d2;
      --accent-ink: #ab9eff;
      --practice-tint: color-mix(in oklch, var(--accent-fill) 16%, var(--surface));
      --practice-tint-edge: #473983;
      --good: #24b47e;
      --critical: #ff6b6b;
      --shadow-card: 0 1px 2px rgb(0 0 0 / 30%), 0 8px 24px rgb(0 0 0 / 24%);
    }
  }
  * { box-sizing: border-box; }
  body {
    min-height: 100vh;
    margin: 0;
    background: var(--page);
    color: var(--ink);
    font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  }
  main { width: min(100%, 52rem); margin: 0 auto; padding: 2.5rem 1.25rem; }
  header { margin-bottom: 2rem; }
  .brand { font-size: 1.125rem; font-weight: 700; letter-spacing: -0.02em; }
  .hero { margin-bottom: 2rem; }
  .badge {
    display: inline-block;
    margin-bottom: 1rem;
    padding: 0.375rem 0.875rem;
    border: 1px solid var(--practice-tint-edge);
    border-radius: 999px;
    background: var(--practice-tint);
    color: var(--accent-ink);
    font-size: 0.8125rem;
    font-weight: 600;
  }
  h1 {
    margin: 0;
    font-size: clamp(2rem, 5vw, 3.25rem);
    line-height: 1.1;
    letter-spacing: -0.035em;
  }
  .lead { margin: 0.875rem 0 0; color: var(--sub); font-size: 1.125rem; line-height: 1.55; }
  form {
    padding: 1.5rem;
    border: 1px solid var(--hairline);
    border-radius: 20px;
    background: var(--surface);
    box-shadow: var(--shadow-card);
  }
  fieldset { min-width: 0; margin: 0; padding: 0; border: 0; }
  fieldset + fieldset { margin-top: 1.5rem; padding-top: 1.5rem; border-top: 1px solid var(--hairline); }
  legend { padding: 0; font-size: 1.25rem; font-weight: 700; letter-spacing: -0.02em; }
  .requirement {
    margin-left: 0.5rem;
    color: var(--accent-ink);
    font-size: 0.75rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }
  .group-note { margin: 0.5rem 0 1.25rem; color: var(--sub); line-height: 1.5; }
  .field + .field, .field-row { margin-top: 1rem; }
  .field-row { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; }
  .field-row > .field + .field { margin-top: 0; }
  label { display: block; margin-bottom: 0.375rem; font-size: 0.875rem; font-weight: 600; }
  input {
    display: block;
    width: 100%;
    min-height: 2.5rem;
    padding: 0.5rem 0.75rem;
    border: 1px solid var(--control-border);
    border-radius: 8px;
    background: var(--surface);
    color: var(--ink);
    font: inherit;
  }
  small { display: block; margin-top: 0.375rem; color: var(--sub); font-size: 0.8125rem; line-height: 1.5; }
  a { color: var(--accent-ink); }
  :focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
  .action-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem 1rem;
    margin-top: 1.5rem;
    padding-top: 1.5rem;
    border-top: 1px solid var(--hairline);
  }
  button {
    min-height: 2.75rem;
    padding: 0.75rem 1rem;
    border: 1px solid var(--accent);
    border-radius: 8px;
    background: var(--accent-fill);
    color: #ffffff;
    cursor: pointer;
    font: inherit;
    font-weight: 700;
    transition: background 120ms ease;
  }
  button:not(:disabled):hover { background: var(--accent-deep); }
  button.secondary { background: transparent; color: var(--accent-ink); }
  button.secondary:not(:disabled):hover { background: var(--practice-tint); }
  .import-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem 1rem;
    margin-bottom: 1.5rem;
    padding-bottom: 1.5rem;
    border-bottom: 1px solid var(--hairline);
  }
  .import-row small { flex: 1 1 14rem; margin-top: 0; }
  /* The button opens it; the rule for every input above would show it. */
  .import-row input[type="file"] { display: none; }
  .offer-row:not([hidden]) {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem 1rem;
    margin-bottom: 1.5rem;
    padding-bottom: 1.5rem;
    border-bottom: 1px solid var(--hairline);
  }
  #offer-text { flex: 1 1 14rem; min-width: 0; margin: 0; overflow-wrap: anywhere; }
  #import-status, #offer-status { flex-basis: 100%; margin-top: 0; }
  #import-status.error, #offer-status.error { color: var(--critical); }
  #import-status.ok, #offer-status.ok { color: var(--good); }
  button:disabled { cursor: not-allowed; opacity: 0.6; }
  #status { flex: 1 1 14rem; min-width: 0; overflow-wrap: anywhere; white-space: pre-wrap; }
  #status.error { color: var(--critical); }
  #status.ok { color: var(--good); }
  @media (max-width: 600px) {
    main { padding: 1.25rem; }
    header { margin-bottom: 1.25rem; }
    .hero { margin-bottom: 1.5rem; }
    .lead { font-size: 1rem; }
    form { padding: 1.25rem; }
    .field-row { grid-template-columns: minmax(0, 1fr); }
    input { font-size: 1rem; }
    .action-row { align-items: stretch; }
    .action-row button { width: 100%; }
  }
</style>
</head>
<body>
<main>
  <header><div class="brand">CODETRIAL</div></header>
  <section class="hero" aria-labelledby="setup-title">
    <span class="badge">First-time setup</span>
    <h1 id="setup-title">Set up CodeTrial</h1>
    <p class="lead">Connect your services to start practicing live technical interviews.</p>
  </section>
  <form id="setup-form">
    <div id="offer" class="offer-row" hidden>
      <p id="offer-text"></p>
      <button id="use-offer" type="button">Use these keys</button>
      <small id="offer-status" role="status" aria-live="polite"></small>
    </div>
    <div class="import-row">
      <button id="import" class="secondary" type="button">Import from a config file</button>
      <input id="import-file" type="file" hidden>
      <small>Fills the fields below from a codetrial.env.local saved earlier. Nothing is sent until you save.</small>
      <small id="import-status" role="status" aria-live="polite"></small>
    </div>
    <fieldset>
      <legend>LiveKit credentials <span class="requirement">Required</span></legend>
      <p class="group-note">Use the URL and keys from the same LiveKit project.</p>
      <div class="field">
        <label for="livekitUrl">LiveKit URL</label>
        <input id="livekitUrl" name="livekitUrl" type="text" placeholder="wss://your-project.livekit.cloud" required>
        <small>Find it at <a href="https://cloud.livekit.io" target="_blank" rel="noopener">cloud.livekit.io</a>.</small>
      </div>
      <div class="field-row">
        <div class="field">
          <label for="livekitApiKey">LiveKit API Key</label>
          <input id="livekitApiKey" name="livekitApiKey" type="text" required>
        </div>
        <div class="field">
          <label for="livekitApiSecret">LiveKit API Secret</label>
          <input id="livekitApiSecret" name="livekitApiSecret" type="password" required>
        </div>
      </div>
      <small>Find both on the project's Keys page.</small>
    </fieldset>
    <fieldset>
      <legend>Google AI <span class="requirement">Optional</span></legend>
      <p class="group-note">Add a key if this server will host the interviewer.</p>
      <div class="field">
        <label for="googleApiKey">Google API Key</label>
        <input id="googleApiKey" name="googleApiKey" type="password">
        <small>Get a key from <a href="https://aistudio.google.com/apikey" target="_blank" rel="noopener">aistudio.google.com</a>.</small>
        <small>Without a key, the app still loads and an interview can start, but an interviewer running elsewhere must join the room.</small>
      </div>
    </fieldset>
    <div class="action-row">
      <button id="submit" type="submit">Save and continue</button>
      <div id="status" role="status" aria-live="polite"></div>
    </div>
  </form>
</main>
<script>
  const form = document.getElementById('setup-form');
  const status = document.getElementById('status');
  const submit = document.getElementById('submit');
  const importButton = document.getElementById('import');
  const importFile = document.getElementById('import-file');
  const importStatus = document.getElementById('import-status');
  const offer = document.getElementById('offer');
  const offerText = document.getElementById('offer-text');
  const useOffer = document.getElementById('use-offer');
  const offerStatus = document.getElementById('offer-status');

  // The fields a config file can fill, by the key the file spells them with.
  const IMPORTED = {
    LIVEKIT_URL: 'livekitUrl',
    LIVEKIT_API_KEY: 'livekitApiKey',
    LIVEKIT_API_SECRET: 'livekitApiSecret',
    GOOGLE_API_KEY: 'googleApiKey',
  };

  // The rules of read_config_file in src/config.rs, so a file the server reads
  // fills the form with what the server would read from it: blank lines and #
  // comments skipped, every other line KEY=value, both sides trimmed, and a
  // quote stripped from either end of the value. A line that is not KEY=value
  // is refused, as the server refuses it, rather than skipped.
  function parseConfig(text) {
    const values = {};
    for (const line of text.split(/\r?\n/)) {
      const trimmed = line.trim();
      if (trimmed === '' || trimmed.startsWith('#')) continue;
      const equals = trimmed.indexOf('=');
      if (equals < 0) throw new Error('not a KEY=value line: ' + trimmed);
      values[trimmed.slice(0, equals).trim()] = trimmed
        .slice(equals + 1)
        .trim()
        .replace(/^"+|"+$/g, '')
        .replace(/^'+|'+$/g, '');
    }
    return values;
  }

  function showImport(className, text) {
    importStatus.className = className;
    importStatus.textContent = text;
  }

  importButton.addEventListener('click', () => importFile.click());
  importFile.addEventListener('change', async () => {
    const file = importFile.files[0];
    // Cleared so that choosing the same file again, after editing it, reads it
    // again instead of firing no change at all.
    importFile.value = '';
    if (!file) return;
    let values;
    try {
      values = parseConfig(await file.text());
    } catch (error) {
      showImport('error', file.name + ': ' + error.message);
      return;
    }
    if (!['LIVEKIT_URL', 'LIVEKIT_API_KEY', 'LIVEKIT_API_SECRET'].every((key) => values[key])) {
      showImport('error', file.name + ' does not set LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET.');
      return;
    }

    // GOOGLE_API_KEYS outranks GOOGLE_API_KEY, as it does when the server reads
    // the file. The form saves one key, so a rotation list gives its first and
    // says so; the rest can be added to the saved config later.
    let note = '';
    const rotation = (values.GOOGLE_API_KEYS || '').split(',').map((key) => key.trim()).filter(Boolean);
    if (rotation.length > 0) {
      values.GOOGLE_API_KEY = rotation[0];
      if (rotation.length > 1) {
        note = ' GOOGLE_API_KEYS lists ' + rotation.length +
          ' keys; Setup saves the first, and the rest can be added to the saved config.';
      }
    }
    for (const [key, name] of Object.entries(IMPORTED)) {
      form.elements[name].value = values[key] || '';
    }
    showImport('ok', 'Filled from ' + file.name + '. Check the values, then save.' + note);
  });

  async function waitForRestart() {
    // The server keeps the port and swaps what answers on it, so a request landing in
    // the handover waits in the accept backlog rather than failing. Poll anyway: the
    // gap is not zero. Any route the full app alone answers would do; /api/session is
    // one, and it is the route this page is on its way to.
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

  // Both ways to save go through here: the form's fields, or `{ useOffer: true }`,
  // which the server answers from the offered config without the keys passing
  // through this page. `line` is the status next to whichever was pressed.
  async function save(data, line) {
    // A second submission would reach a config file the first one has already
    // written, and the write refuses to overwrite: the double-click would report
    // a failure over a setup that worked. Re-enabled only where the form is
    // still the way forward, which a successful submission is not.
    submit.disabled = true;
    useOffer.disabled = true;
    line.className = '';
    line.textContent = 'Checking credentials…';
    let response;
    try {
      response = await fetch('/api/setup', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(data),
      });
    } catch (error) {
      line.className = 'error';
      line.textContent = 'Request failed: ' + error;
      submit.disabled = false;
      useOffer.disabled = false;
      return;
    }
    const body = await response.json().catch(() => ({}));
    if (response.ok) {
      line.className = 'ok';
      line.textContent = 'Saved. Starting codetrial…';
      waitForRestart();
    } else {
      line.className = 'error';
      line.textContent = body.error || ('Request failed: ' + response.status);
      submit.disabled = false;
      useOffer.disabled = false;
    }
  }

  form.addEventListener('submit', (event) => {
    event.preventDefault();
    save(Object.fromEntries(new FormData(form).entries()), status);
  });
  useOffer.addEventListener('click', () => save({ useOffer: true }, offerStatus));

  // A config the launcher found, such as the CLI's: named here with its LiveKit
  // host so the person can tell whose it is before using it. A 404 is no offer.
  (async () => {
    let response;
    try {
      response = await fetch('/api/setup/offer', { cache: 'no-store' });
    } catch (error) {
      return;
    }
    if (!response.ok) return;
    const found = await response.json().catch(() => null);
    if (!found) return;
    if (found.error) {
      showImport('error', found.error);
      return;
    }
    offerText.textContent = 'Keys found in ' + found.path + ': LiveKit ' + found.livekitEndpoint +
      (found.google ? ', with a Google key.' : ', without a Google key.') +
      (found.note ? ' ' + found.note : '');
    offer.hidden = false;
  })();
</script>
</body>
</html>
"#;

/// The four values a submission carries, trimmed and checked.
struct SetupFields {
    livekit_url: String,
    livekit_api_key: String,
    livekit_api_secret: String,
    google_api_key: String,
}

impl SetupFields {
    /// The keys this submission becomes, in the order the file writes them.
    ///
    /// One list, hanging off the values it is derived from: what the launch is
    /// probed with has to be what gets written, or the answer was about a
    /// different config.
    ///
    /// A blank `googleApiKey` leaves the key out rather than writing it empty.
    /// `read_config_file` keeps an empty value, and `load_values` lays file
    /// pairs over the environment, so the line would erase a `GOOGLE_API_KEY`
    /// the operator had exported and drop the process to web-only -- announced
    /// on a console a double-clicked binary does not have. A hand-written
    /// config omits the key to defer to the environment, and this writes the
    /// same file.
    fn pairs(&self) -> Vec<(&str, &str)> {
        let mut pairs = vec![
            ("LIVEKIT_URL", self.livekit_url.as_str()),
            ("LIVEKIT_API_KEY", self.livekit_api_key.as_str()),
            ("LIVEKIT_API_SECRET", self.livekit_api_secret.as_str()),
        ];
        if !self.google_api_key.is_empty() {
            pairs.push(("GOOGLE_API_KEY", self.google_api_key.as_str()));
        }
        pairs
    }

    /// The file, from `pairs`, so it holds exactly the config that was checked.
    /// Every value is known to carry no newline by now, which is what makes one
    /// line per key a faithful encoding of it.
    fn contents(&self) -> String {
        self.pairs()
            .iter()
            .map(|(key, value)| format!("{key}={value}\n"))
            .collect()
    }
}

/// Why `value` cannot be written as a `KEY=value` line of the config file and
/// read back as itself, if it cannot.
///
/// These are read back by `read_config_file`, which parses one line at a
/// time: a newline inside a value is a second line, and a second line is a
/// config key nobody submitted. `SESSION_SECRET` is one such key, and this
/// process reads the file it just wrote.
fn unwritable(key: &str, value: &str) -> Option<String> {
    if value.chars().any(char::is_control) {
        return Some(format!("{key} must not contain control characters"));
    }

    // `read_config_file` does `trim_matches('"')` then `trim_matches('\'')`, so
    // a value edged with either is probed as one string and loaded as another.
    // Refused rather than written, for the reason the newline above is: this
    // file format cannot carry the value, and answering 200 would mean the file
    // does not hold what was checked. Refusing says so while there is still a
    // form to say it in.
    ['"', '\'']
        .into_iter()
        .find(|quote| value.starts_with(*quote) || value.ends_with(*quote))
        .map(|edge| {
            format!(
                "{key} must not start or end with {edge}: the config file drops a quote at \
                 either end, so the value read back would not be the one checked here"
            )
        })
}

/// Everything a submission can be refused for before anything is sent or
/// written.
///
/// Read out of a `Value`, not a derived struct: this crate has no `serde`
/// derive dependency, and a missing field reads as empty rather than a parse
/// error.
///
/// One function because the three rules answer one question -- can this file
/// hold this value and be read back as the same value -- and they are only
/// correct together. Trimmed here, once, because everything downstream assumes
/// it was. A pasted URL keeps its leading space through `validate_livekit_url`,
/// which trims to decide and hands the original on, and `livekit_scheme` then
/// matches at offset 0 and rewrites nothing: working credentials come back as
/// "did not work". A key with a trailing space is signed into the JWT verbatim
/// and refused, though the same value hand-written into the config file works,
/// because `read_config_file` trims what it reads. And a `googleApiKey` of
/// spaces is not empty to `is_empty`, so it takes the branch that probes Gemini
/// and fails there for a field the form calls optional.
fn validated_fields(submission: &Value) -> Result<SetupFields, String> {
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
            return Err(format!("{key} is required"));
        }
    }

    // Every field, including the optional one, and before anything is sent
    // anywhere.
    for key in [
        "livekitUrl",
        "livekitApiKey",
        "livekitApiSecret",
        "googleApiKey",
    ] {
        if let Some(error) = unwritable(key, &field(key)) {
            return Err(error);
        }
    }

    Ok(SetupFields {
        livekit_url: field("livekitUrl"),
        livekit_api_key: field("livekitApiKey"),
        livekit_api_secret: field("livekitApiSecret"),
        google_api_key: field("googleApiKey"),
    })
}

/// The config file Setup would write, from credentials that are already in
/// `values` (a launch's environment), so a launch that has them is not asked
/// for them again. `None` unless all three LiveKit keys are there, which leaves
/// the asking to Setup.
///
/// The same checks as a submission, the format's and the URL's, but not the
/// probes: those reach the network, and a value an operator exported is taken
/// as a hand-written config file would be, and refused by the launch the same
/// way if it is wrong.
pub fn config_from_values(
    values: &std::collections::BTreeMap<String, String>,
    production: bool,
) -> Result<Option<String>, String> {
    let value = |key: &str| values.get(key).map_or("", |value| value.trim());
    if ["LIVEKIT_URL", "LIVEKIT_API_KEY", "LIVEKIT_API_SECRET"]
        .iter()
        .any(|key| value(key).is_empty())
    {
        return Ok(None);
    }
    let fields = validated_fields(&json!({
        "livekitUrl": value("LIVEKIT_URL"),
        "livekitApiKey": value("LIVEKIT_API_KEY"),
        "livekitApiSecret": value("LIVEKIT_API_SECRET"),
        "googleApiKey": value("GOOGLE_API_KEY"),
    }))?;
    crate::config::validate_livekit_url(&fields.livekit_url, production)?;

    // Setup's form takes one Google key, but an operator may export the
    // rotation list instead, and dropping it would leave the install web-only.
    let mut contents = fields.contents();
    let keys = value("GOOGLE_API_KEYS");
    if !keys.is_empty() {
        if let Some(error) = unwritable("GOOGLE_API_KEYS", keys) {
            return Err(error);
        }

        // The rule the launch applies to the list, applied before the file
        // exists: a list it refuses, once written, would refuse every later
        // launch too, and the file's existence would keep Setup from coming
        // back to correct it. The list alone, so an exported `GOOGLE_API_KEY`
        // beside it is not reported as overridden here.
        crate::config::google_api_keys(&std::collections::BTreeMap::from([(
            "GOOGLE_API_KEYS".to_string(),
            keys.to_string(),
        )]))?;
        contents.push_str(&format!("GOOGLE_API_KEYS={keys}\n"));
    }
    Ok(Some(contents))
}

/// Proves the credentials before a file is written from them.
///
/// `Some` is the refusal. The order is the point: the URL rule is local and
/// instant, the LiveKit probe is one round trip, and the Gemini probe opens a
/// live session, so a submission wrong in the cheapest way is not made to wait
/// for the most expensive check to say so.
async fn probe_credentials(fields: &SetupFields, production: bool) -> Option<Response> {
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
    if let Err(error) = crate::config::validate_livekit_url(&fields.livekit_url, production) {
        return Some(super::json_response(
            StatusCode::BAD_REQUEST,
            json!({ "error": error }),
        ));
    }

    if let Err(error) = crate::livekit::validate_livekit_credentials(
        &fields.livekit_url,
        &fields.livekit_api_key,
        &fields.livekit_api_secret,
        crate::current_epoch_seconds(),
    )
    .await
    {
        return Some(super::json_response(
            StatusCode::BAD_REQUEST,
            json!({
                "error": format!(
                    "livekitUrl, livekitApiKey or livekitApiSecret did not work: {error}"
                )
            }),
        ));
    }

    if fields.google_api_key.is_empty() {
        return None;
    }
    let config = match load_from_pairs(fields.pairs().iter().copied()) {
        Ok(config) => config,
        Err(error) => {
            return Some(super::json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": error.to_string() }),
            ));
        }
    };

    // Same proof as `check-gemini`: open a real session.
    // `CODETRIAL_GEMINI_LIVE_URL` lets tests redirect this away from the real
    // endpoint.
    let room_name = format!("{}-smoke", config.room_prefix);
    let boot = bootstrap(&config, &room_name, None, config.default_duration_min);
    let url = std::env::var("CODETRIAL_GEMINI_LIVE_URL")
        .unwrap_or_else(|_| gemini_live_websocket_url(&fields.google_api_key));
    match open_live_session_at(&url, &boot, None).await {
        Ok(session) => {
            let _ = session.close().await;
            None
        }
        Err(error) => {
            // Redacted like every other caller of this chain. The close frame's
            // reason is folded in at the bottom of it, so the text here is
            // partly Gemini's, and the key was sent in the URL.
            let reason = crate::gemini::redact_api_key(
                &format!("googleApiKey did not work: {error}"),
                &fields.google_api_key,
            );
            Some(super::json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": reason }),
            ))
        }
    }
}

/// Writes the config file, and refuses rather than overwriting one.
///
/// The path is `config_path`, which `primary_config_path` searches for: the
/// file has to be found again on the next launch, from whatever directory that
/// launch happens to start in.
///
/// `Some` is the refusal. The file holds `LIVEKIT_API_SECRET` and
/// `GOOGLE_API_KEY`, hence `create_owner_only`.
fn write_config_file(path: &Path, contents: &str) -> Option<Response> {
    // The directory is the caller's answer, and a released binary's copy of it
    // does not exist until the first submission.
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        return Some(super::json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("could not create {}: {error}", parent.display()) }),
        ));
    }

    let Err(error) = crate::create_owner_only(path, contents.as_bytes()) else {
        return None;
    };
    let reason = if error.kind() == std::io::ErrorKind::AlreadyExists {
        format!(
            "{} already exists; move it aside and reload",
            path.display()
        )
    } else {
        format!("could not write {}: {error}", path.display())
    };
    Some(super::json_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({ "error": reason }),
    ))
}

/// One submission: checked, probed, written, and the app let out behind it.
/// The keys in an offered config, as the form would submit them, with what of
/// the LiveKit URL the page may show and a note for it.
///
/// Read the way the server reads any config, then cut down to what the form
/// takes, so using it is a submission like a typed one and is checked the
/// same way. The form takes one Google key, so a `GOOGLE_API_KEYS` list, which
/// outranks `GOOGLE_API_KEY` as it does when the server reads the file, gives
/// its first, as importing the file into the form does.
fn read_offer(path: &Path) -> Result<(Value, String, Option<String>), String> {
    let shown = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|error| format!("{shown}: {error}"))?;

    // By number, never by its text: this reaches the page, which any account on
    // the machine can ask for, and a malformed line can hold a secret.
    let file = crate::config::parse_config(&text)
        .map_err(|number| format!("{shown}: line {number} is not KEY=value"))?;
    let mut values = std::collections::BTreeMap::new();
    crate::config::apply_config_file(&mut values, file);
    let value = |key: &str| values.get(key).map_or("", |value| value.trim());
    if ["LIVEKIT_URL", "LIVEKIT_API_KEY", "LIVEKIT_API_SECRET"]
        .iter()
        .any(|key| value(key).is_empty())
    {
        return Err(format!(
            "{shown} does not set LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET."
        ));
    }

    // Validated before anything of it is shown, by a check whose messages never
    // name the URL, and then shown as its scheme and host alone: the user
    // information and the query are where a URL carries a credential.
    let url = value("LIVEKIT_URL");
    crate::config::validate_livekit_url(url, false).map_err(|error| format!("{shown}: {error}"))?;
    let endpoint = format!(
        "{}{}",
        crate::config::livekit_scheme(url).map_or("", |(scheme, _, _)| scheme),
        crate::config::livekit_authority(url)
    );
    let rotation = value("GOOGLE_API_KEYS")
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    let note = (rotation.len() > 1).then(|| {
        format!(
            "GOOGLE_API_KEYS lists {} keys; Setup saves the first, and the rest can be added \
             to the saved config.",
            rotation.len()
        )
    });
    let submission = json!({
        "livekitUrl": value("LIVEKIT_URL"),
        "livekitApiKey": value("LIVEKIT_API_KEY"),
        "livekitApiSecret": value("LIVEKIT_API_SECRET"),
        "googleApiKey": rotation.first().copied().unwrap_or(value("GOOGLE_API_KEY")),
    });
    Ok((submission, endpoint, note))
}

/// What the page shows of an offered config: its path, its LiveKit scheme and
/// host and whether it holds a Google key, or why it cannot be used. Never a
/// key.
fn offer_summary(offer: Option<&Path>) -> Response {
    let Some(path) = offer else {
        return super::json_response(StatusCode::NOT_FOUND, json!({}));
    };
    let path_shown = path.display().to_string();
    match read_offer(path) {
        Ok((submission, endpoint, note)) => super::json_response(
            StatusCode::OK,
            json!({
                "path": path_shown,
                "livekitEndpoint": endpoint,
                "google": submission["googleApiKey"].as_str().is_some_and(|key| !key.is_empty()),
                "note": note,
            }),
        ),
        Err(error) => super::json_response(
            StatusCode::OK,
            json!({ "path": path_shown, "error": error }),
        ),
    }
}

async fn submit_setup(
    Json(submission): Json<Value>,
    ready: Arc<Notify>,
    production: bool,
    path: PathBuf,
    offer: Option<PathBuf>,
) -> Response {
    // Using the offered config is a submission the server fills in itself, so
    // the keys never pass through the page.
    let submission = if submission.get("useOffer") == Some(&Value::Bool(true)) {
        let Some(offer) = offer else {
            return super::json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "no config was offered to use" }),
            );
        };
        match read_offer(&offer) {
            Ok((submission, _, _)) => submission,
            Err(error) => {
                return super::json_response(StatusCode::BAD_REQUEST, json!({ "error": error }));
            }
        }
    } else {
        submission
    };
    let fields = match validated_fields(&submission) {
        Ok(fields) => fields,
        Err(error) => {
            return super::json_response(StatusCode::BAD_REQUEST, json!({ "error": error }));
        }
    };

    if let Some(refusal) = probe_credentials(&fields, production).await {
        return refusal;
    }

    if let Some(refusal) = write_config_file(&path, &fields.contents()) {
        return refusal;
    }

    // Only reached once the file is on disk; tells the caller to stop serving
    // Setup and continue as the full app.
    ready.notify_one();
    super::json_response(StatusCode::OK, json!({ "saved": true }))
}

#[cfg(test)]
#[path = "../../tests/unit/web/setup.rs"]
mod tests;

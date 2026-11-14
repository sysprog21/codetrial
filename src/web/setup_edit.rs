//! Change keys: the Setup page served again by a running server, so its
//! LiveKit and Google keys can be replaced without editing the config file by
//! hand. A save rewrites the file and nothing else. This process keeps the
//! keys it started with, for every interview it starts, until someone restarts
//! it and the new keys arrive through the same startup path the first ones did.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use super::{AppState, RoomDispatcher, json_response};

/// What Change keys needs of the launch serving it. No `Debug`: it holds keys.
pub struct SetupContext {
    /// The config file this launch read, and what it held then.
    path: PathBuf,
    contents: String,
    /// What this launch runs on: the environment, that file and the flags.
    values: BTreeMap<String, String>,
    port: u16,
    code: String,
    /// One save at a time. The next one finds the file changed and is refused.
    saving: tokio::sync::Mutex<()>,
}

impl SetupContext {
    /// `None` unless this launch may rewrite its own config.
    ///
    /// The loopback bind keeps the network out and the `Host` check keeps out
    /// a page that rebinds a name to 127.0.0.1, but any account on this machine
    /// reaches the port and chooses every header itself. What such an account
    /// lacks is the code, new on every start: `run_web` makes it and prints it
    /// to the terminal of whoever started the server, or takes it from the
    /// desktop app that launched this one as its child, which has no terminal
    /// anyone reads and shows the code in its own window instead. `code` is
    /// `None` for a launch that has neither. Behind a proxy the requests are
    /// not only this machine's, so that launch gets no Change keys either.
    ///
    /// A config that is a symlink is not offered either: renaming the new file
    /// over it would replace the link and leave its target unchanged.
    pub fn new(
        path: PathBuf,
        values: BTreeMap<String, String>,
        bound: SocketAddr,
        code: Option<String>,
        trusted_proxy_hops: u32,
    ) -> Option<Self> {
        let code = code.filter(|_| bound.ip().is_loopback() && trusted_proxy_hops == 0)?;
        let contents = regular_file(&path)?;
        Some(Self {
            path,
            contents,
            values,
            port: bound.port(),
            code,
            saving: tokio::sync::Mutex::new(()),
        })
    }

    /// For `run_web` to print, and for nothing else.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// What the page may show of the running config. Never a key.
    fn summary(&self) -> Value {
        let value = |key: &str| self.values.get(key).map_or("", |value| value.trim());
        json!({
            "configured": true,
            "livekitEndpoint": super::setup::public_endpoint(value("LIVEKIT_URL")),
            "googleApiKey": !value("GOOGLE_API_KEY").is_empty() || !value("GOOGLE_API_KEYS").is_empty(),
            "restartPending": regular_file(&self.path).as_deref() != Some(self.contents.as_str()),
        })
    }

    async fn save(&self, submission: &Value, dispatcher: Option<&dyn RoomDispatcher>) -> Response {
        // Digests compared rather than the codes, so how long a wrong guess
        // takes to refuse says nothing about how much of it was right.
        let given = submission.get("code").and_then(Value::as_str).unwrap_or("");
        let digest = |code: &str| ring::digest::digest(&ring::digest::SHA256, code.as_bytes());
        if digest(given).as_ref() != digest(&self.code).as_ref() {
            return json_response(
                StatusCode::FORBIDDEN,
                json!({ "error": "Enter the Change keys code this server printed when it started, or the one the desktop app that started it shows." }),
            );
        }
        let _saving = self.saving.lock().await;

        // The new keys are merged into the file as this launch read it, which
        // is what the running config was validated from. Once the file differs,
        // by a save here or an edit elsewhere, it is not what this process runs
        // on, and only a restart makes it so again.
        if regular_file(&self.path).as_deref() != Some(self.contents.as_str()) {
            return json_response(
                StatusCode::CONFLICT,
                json!({
                    "error": "The config file has changed since this server started. Restart the \
                              server before changing keys again."
                }),
            );
        }

        // Read the way a first-time submission is, a missing or blank field
        // being one the file keeps.
        let mut replacements = Vec::new();
        for (field, key) in [
            ("livekitUrl", "LIVEKIT_URL"),
            ("livekitApiKey", "LIVEKIT_API_KEY"),
            ("livekitApiSecret", "LIVEKIT_API_SECRET"),
            ("googleApiKey", "GOOGLE_API_KEY"),
        ] {
            let value = submission
                .get(field)
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if value.is_empty() {
                continue;
            }
            if let Some(error) = super::setup::unwritable(field, value) {
                return json_response(StatusCode::BAD_REQUEST, json!({ "error": error }));
            }
            replacements.push((key, value));
        }
        let google_api_key = replacements
            .iter()
            .find(|(key, _)| *key == "GOOGLE_API_KEY")
            .map_or("", |(_, value)| *value);
        if !google_api_key.is_empty() {
            // The list outranks the single key, so it is cleared rather than
            // left to win over the key that was just entered.
            replacements.push(("GOOGLE_API_KEYS", ""));
        }
        let contents = merge(&self.contents, &replacements);
        if contents == self.contents {
            return json_response(
                StatusCode::BAD_REQUEST,
                json!({ "error": "Nothing to change: every field is blank or matches the config file." }),
            );
        }

        let mut values = self.values.clone();
        crate::config::apply_config_file(
            &mut values,
            replacements
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        );
        let production = crate::config::is_production(&values);
        if let Some(refusal) =
            super::setup::probe_credentials(&values, google_api_key, production).await
        {
            return refusal;
        }

        // Last, because an interview can start and the file can be edited while
        // the probes run, which can take seconds.
        if dispatcher.is_some_and(|dispatcher| !dispatcher.is_idle()) {
            return json_response(
                StatusCode::CONFLICT,
                json!({
                    "error": "An interview is running on this server or still writing its report. \
                              Save again once it has ended."
                }),
            );
        }
        if regular_file(&self.path).as_deref() != Some(self.contents.as_str()) {
            return json_response(
                StatusCode::CONFLICT,
                json!({
                    "error": "The config file changed while the new keys were being checked. \
                              Restart the server before changing keys again."
                }),
            );
        }
        if let Err(error) = replace_file(&self.path, &contents) {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": format!("could not write {}: {error}", self.path.display()) }),
            );
        }
        json_response(StatusCode::OK, json!({ "saved": true }))
    }
}

/// The file's text, when `path` is a regular file and not a symlink to one.
fn regular_file(path: &Path) -> Option<String> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .and_then(|_| std::fs::read_to_string(path).ok())
}

/// `contents` with every `KEY=value` line of a replaced key rewritten and a
/// line appended for a key it does not set yet. Everything else, comments
/// included, stays as written. A blank replacement only clears: appended, it
/// would be a line that sets nothing.
fn merge(contents: &str, replacements: &[(&str, &str)]) -> String {
    // The byte order mark is the file's own, and `parse_config` reads past it.
    let (mark, text) = contents
        .strip_prefix('\u{feff}')
        .map_or(("", contents), |text| ("\u{feff}", text));
    let mut merged = mark.to_string();
    let mut rewritten = Vec::new();
    for line in text.split_inclusive('\n') {
        let entry = line.trim();
        let replacement = entry
            .split_once('=')
            .filter(|_| !entry.starts_with('#'))
            .and_then(|(key, _)| {
                replacements
                    .iter()
                    .find(|(name, _)| *name == crate::config::config_key(key))
            });
        if let Some((key, value)) = replacement {
            let ending = &line[line.trim_end_matches(['\r', '\n']).len()..];
            let export = if crate::config::config_key(entry) != entry {
                "export "
            } else {
                ""
            };
            merged.push_str(&format!("{export}{key}={value}{ending}"));
            rewritten.push(*key);
        } else {
            merged.push_str(line);
        }
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    for (key, value) in replacements {
        if value.is_empty() || rewritten.contains(key) {
            continue;
        }
        if merged.len() > mark.len() && !merged.ends_with('\n') {
            merged.push_str(newline);
        }
        merged.push_str(&format!("{key}={value}{newline}"));
    }
    merged
}

/// Writes `contents` over the config at `path` with no moment where the file
/// is missing or half written: into a new file beside it, renamed over it.
///
/// The new file is owner-only from before a key is in it, which on Unix is the
/// mode Setup writes the first config with. On Windows a file takes its
/// folder's ACL, which can admit accounts the config itself kept out, so it is
/// given a protected DACL naming its owner alone instead.
fn replace_file(path: &Path, contents: &str) -> std::io::Result<()> {
    let temporary = path.with_file_name(format!(
        ".codetrial-setup-{}.tmp",
        crate::accounts::random_token(12)?
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        // GENERIC_WRITE, READ_CONTROL and WRITE_DAC, so this handle can set its
        // own DACL, and no sharing, so no other handle opens the file first.
        use std::os::windows::fs::OpenOptionsExt;
        options.access_mode(0x4006_0000).share_mode(0);
    }
    let mut file = options.open(&temporary)?;
    let written = write_owner_only(&mut file, contents);

    // Windows renames or deletes a file only once this handle is closed.
    drop(file);
    let replaced = written.and_then(|()| std::fs::rename(&temporary, path));
    if replaced.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    replaced
}

fn write_owner_only(file: &mut File, contents: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    owner_only_dacl(file)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

#[cfg(windows)]
fn owner_only_dacl(file: &mut File) -> std::io::Result<()> {
    use windows_permissions::constants::{SeObjectType, SecurityInformation};
    use windows_permissions::wrappers::{GetSecurityInfo, SetSecurityInfo};
    use windows_permissions::{LocalBox, SecurityDescriptor};

    let created = GetSecurityInfo(
        &*file,
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Owner,
    )?;
    let owner = created
        .owner()
        .ok_or_else(|| std::io::Error::other("the new config file has no owner"))?;
    let owner_only: LocalBox<SecurityDescriptor> = format!("D:P(A;;FA;;;{owner})").parse()?;
    SetSecurityInfo(
        file,
        SeObjectType::SE_FILE_OBJECT,
        SecurityInformation::Dacl | SecurityInformation::ProtectedDacl,
        None,
        None,
        owner_only.dacl(),
        None,
    )
}

/// The context, for a request this launch answers: one whose `Host` names the
/// loopback port, for the reason first-time Setup checks it, in
/// `setup::host_is_loopback`.
#[expect(
    clippy::result_large_err,
    reason = "the error is the response sent to the client"
)]
fn allowed<'a>(state: &'a AppState, headers: &HeaderMap) -> Result<&'a SetupContext, Response> {
    let Some(context) = state.setup.as_deref() else {
        return Err(StatusCode::NOT_FOUND.into_response());
    };
    if super::setup::host_is_loopback(headers.get(header::HOST), context.port) {
        Ok(context)
    } else {
        Err(json_response(
            StatusCode::FORBIDDEN,
            json!({
                "error": "Change keys answers only to a loopback Host. Open it at the address the \
                          console printed."
            }),
        ))
    }
}

pub(super) async fn page_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match allowed(&state, &headers) {
        Ok(_) => super::setup::setup_page().await.into_response(),
        Err(refusal) => refusal,
    }
}

pub(super) async fn summary_handler(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match allowed(&state, &headers) {
        Ok(context) => json_response(StatusCode::OK, context.summary()),
        Err(refusal) => refusal,
    }
}

pub(super) async fn save_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(submission): Json<Value>,
) -> Response {
    match allowed(&state, &headers) {
        Ok(context) => context.save(&submission, state.dispatcher.as_deref()).await,
        Err(refusal) => refusal,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/web/setup_edit.rs"]
mod tests;

//! Which URLs the window may show, and what happens to the rest.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use tauri::Url;

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The server this app started.
    Server,
    /// The app's own starting page. It also talks to the app this way, by
    /// navigating to itself with a query the app intercepts, so it has nothing
    /// native to call. Only this page can: the server's own `?reopen` is just a
    /// server page.
    StartPage(Ask),
    /// Any other web page: the default browser shows it instead, so a link the
    /// interview carries still works and the window never holds a page whose
    /// script could reach the server as same origin.
    OpenInBrowser,
    Refuse,
}

/// What the starting page's query asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Ask {
    /// To be shown.
    Nothing,
    /// `?reopen`: the server again in the default browser, for a tab that was
    /// closed.
    Reopen,
    /// `?webrtc=yes` or `?webrtc=no`: whether this web view can make the
    /// interview's call.
    WebRtc(bool),
}

/// Scheme, host and port, compared as written. Not `Url::origin`, which is
/// opaque for `tauri://` and so never equal to itself.
pub fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

pub fn judge(url: &Url, server: &Url, start_page: &Url) -> Verdict {
    if same_origin(url, server) {
        Verdict::Server
    } else if same_origin(url, start_page) {
        Verdict::StartPage(ask(url))
    } else if matches!(url.scheme(), "http" | "https") {
        Verdict::OpenInBrowser
    } else {
        Verdict::Refuse
    }
}

/// What a page the window finished loading is handed of the Change keys code.
#[derive(Debug, PartialEq, Eq)]
pub enum ChangeKeysCode {
    /// Filled into the code field of the server's own Setup page.
    Fill,
    /// Shown on the start page left in the window while the interview runs in
    /// the browser, where nothing here can fill a field.
    Show,
}

/// Which pages get the code, and no other: a mistake here either hides Change
/// keys or hands the code to a page that is not the server's Setup page. Built
/// on `judge`, so a page the window may not show never gets it.
pub fn change_keys_code_for(url: &Url, server: &Url, start_page: &Url) -> Option<ChangeKeysCode> {
    match judge(url, server, start_page) {
        Verdict::Server if url.path() == "/setup" => Some(ChangeKeysCode::Fill),
        Verdict::StartPage(Ask::Nothing) if url.query_pairs().any(|(key, _)| key == "browser") => {
            Some(ChangeKeysCode::Show)
        }
        _ => None,
    }
}

/// Where the window's start page loads, `http_scheme` being true on Windows
/// and Android. Tauri serves `WebviewUrl::App("index.html")` at the root of
/// its own origin, `http://tauri.localhost/` there and `tauri://localhost/`
/// elsewhere, since this app sets neither `devUrl` nor `useHttpsScheme`.
///
/// Known before the window loads anything, so a failure the server reports
/// first still reaches the page. Not `window.url()` after the window is
/// built: on Windows WebView2 has not begun loading by then, and the start
/// page itself was judged a stranger, cancelled and sent to the browser. Nor
/// the first navigation, which a failure can come before.
pub fn start_page(http_scheme: bool) -> Url {
    let origin = if http_scheme {
        "http://tauri.localhost/"
    } else {
        "tauri://localhost/"
    };
    Url::parse(origin).expect("the start page origin is a URL")
}

fn ask(url: &Url) -> Ask {
    for (key, value) in url.query_pairs() {
        match &*key {
            "reopen" => return Ask::Reopen,
            "webrtc" => return Ask::WebRtc(value == "yes"),
            _ => {}
        }
    }
    Ask::Nothing
}

/// Where a download is saved: the name the page offered, in `dir`, numbered
/// rather than overwriting a file already there or one already `given` to a
/// download still in flight, which has not written its file yet. Only the
/// last component of the offered name is used, so a name carrying a path
/// cannot leave `dir`.
///
/// `given` rather than creating the file to reserve the name: a web view told
/// to save onto a file that exists may refuse the download instead.
pub fn download_path(dir: &Path, offered: &Path, given: &HashSet<PathBuf>) -> PathBuf {
    let name = offered
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("download");
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    let mut candidate = dir.join(name);
    let mut number = 1;
    while candidate.exists() || given.contains(&candidate) {
        candidate = dir.join(format!("{stem} ({number}){extension}"));
        number += 1;
    }
    candidate
}

#[cfg(test)]
#[path = "../tests/unit/navigation.rs"]
mod tests;

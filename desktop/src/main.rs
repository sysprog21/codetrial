//! CodeTrial as a desktop app: one window showing the `codetrial web` server
//! this process starts, on a loopback port it owns for as long as it runs.
//!
//! The page talks to nothing native. There is no command, capability or
//! plugin permission a page can reach; everything the window does, the
//! process decides from outside the page. docs/desktop.md has the boundary.

// A release build is a GUI program and must not open a console beside its
// window. Debug builds keep it, which is where the server's output goes.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod navigation;
mod server;

use std::collections::HashSet;
use std::io::Write as _;
use std::path::Path;
use std::sync::{LazyLock, Mutex, OnceLock, mpsc};
use std::time::Duration;

use tauri::webview::{DownloadEvent, NewWindowResponse, PermissionKind, PermissionResponse};
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use navigation::{Ask, Verdict};

const WINDOW: &str = "main";

/// Fixed rather than chosen at launch, because the browser keys what the page
/// stores locally, practice history among it, by origin: a port that moved
/// would show an empty history every launch. 3101 so that it does not meet the
/// CLI's default of 3000 on a machine that runs both.
const SERVER_ADDR: &str = "127.0.0.1:3101";

static SERVER_URL: LazyLock<Url> = LazyLock::new(|| {
    Url::parse(&format!("http://{SERVER_ADDR}/")).expect("the server address is a URL")
});

/// How long the server gets to exit after its stdin closes before it is
/// killed.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// How long the page has to answer `CLOSE_PROBE` before the question is put
/// to the person instead.
const PAGE_ANSWER: Duration = Duration::from_secs(5);

/// Asks the page's own leave-page guard, the one `web/before-unload.js` arms
/// while an interview's report is unsaved, by firing the event it listens for.
/// A window close sends no `beforeunload` of its own, so without this a close
/// would discard a live interview without asking.
const CLOSE_PROBE: &str = r#"(() => {
  try {
    const event = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(event);
    return event.defaultPrevented ? "guarded" : "free";
  } catch (_) {
    return "guarded";
  }
})()"#;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for both the server to be ready and the starting page to say
    /// whether this web view has WebRTC. See `open_server`.
    Starting {
        server_ready: bool,
        webrtc: Option<bool>,
    },
    Serving,
    /// Serving, to the default browser rather than to this window, because
    /// this window cannot make the call.
    InBrowser,
    Failed,
}

/// Where a close stands. One value rather than two flags, so "quitting but not
/// closing" is not a state anything can reach.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Close {
    Open,
    /// Being decided; another close request joins it.
    Asking,
    /// Decided; the server's exit is expected and not reported.
    Quitting,
}

struct Shell {
    server: Mutex<Option<server::Server>>,
    phase: Mutex<Phase>,
    close: Mutex<Close>,
    start_page: OnceLock<Url>,
}

impl Shell {
    fn quitting(&self) -> bool {
        *self.close.lock().expect("close lock") == Close::Quitting
    }
}

fn main() {
    let app = tauri::Builder::default()
        // First, so a second launch is turned away before it starts a server of
        // its own on the same port.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window(WINDOW) {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .manage(Shell {
            server: Mutex::new(None),
            phase: Mutex::new(Phase::Starting {
                server_ready: false,
                webrtc: None,
            }),
            close: Mutex::new(Close::Open),
            start_page: OnceLock::new(),
        })
        .setup(|app| {
            open_window(app.handle())?;
            launch(app.handle());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                request_quit(window.app_handle());
            }
        })
        .build(tauri::generate_context!())
        .expect("the CodeTrial window should build");

    app.run(|app, event| match event {
        // Cmd+Q and the like arrive here with no code; `app.exit` below arrives
        // with one and is let through. Held even while a quit is under way: a
        // second Cmd+Q then would end the process before `quit` reaches the
        // kill for a server that ignored its closed stdin, and `request_quit`
        // already ignores a request while one is being decided or carried out.
        RunEvent::ExitRequested { api, code, .. } => {
            if code.is_none() {
                api.prevent_exit();
                request_quit(app);
            }
        }
        RunEvent::Exit => {
            if let Some(server) = app
                .state::<Shell>()
                .server
                .lock()
                .expect("server lock")
                .take()
            {
                server.stop(STOP_GRACE);
            }
        }
        _ => {}
    });
}

fn open_window(app: &AppHandle) -> tauri::Result<()> {
    let downloads = app.path().download_dir().ok();
    let given = Mutex::new(HashSet::new());
    let navigation_app = app.clone();
    let window = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("index.html".into()))
        .title("CodeTrial")
        .inner_size(1280.0, 860.0)
        .min_inner_size(960.0, 640.0)
        .on_navigation(move |url| {
            // Before the start page is recorded the window is loading it, and
            // that first navigation is the only one there is.
            let shell = navigation_app.state::<Shell>();
            let Some(start_page) = shell.start_page.get() else {
                return true;
            };
            match navigation::judge(url, &SERVER_URL, start_page) {
                // The server's port only while this process is serving it
                // there: after a failure, or with the interview in the browser,
                // history or a link must not take the window back to whatever
                // now answers on it.
                Verdict::Server => *shell.phase.lock().expect("phase lock") == Phase::Serving,
                Verdict::StartPage(Ask::Nothing) => true,
                Verdict::StartPage(Ask::Reopen) => {
                    open_in_browser(SERVER_URL.as_str());
                    false
                }
                Verdict::StartPage(Ask::WebRtc(webrtc)) => {
                    if let Phase::Starting { webrtc: known, .. } =
                        &mut *shell.phase.lock().expect("phase lock")
                    {
                        *known = Some(webrtc);
                    }
                    open_server(&navigation_app);
                    false
                }
                Verdict::OpenInBrowser => {
                    open_in_browser(url.as_str());
                    false
                }
                Verdict::Refuse => false,
            }
        })
        // `target="_blank"` and `window.open`. A second webview would be a
        // second window into the same origin with none of the rules above, so a
        // web page goes to the browser and nothing opens here.
        .on_new_window(|url, _| {
            if matches!(url.scheme(), "http" | "https") {
                open_in_browser(url.as_str());
            }
            NewWindowResponse::Deny
        })
        // The camera and the microphone, to the server's page and to nothing
        // else. Allowed rather than left to the webview, which on Linux has no
        // prompt of its own and refuses; the operating system still asks before
        // this app first gets either device.
        .on_permission_request(move |webview, kind| {
            let ours = webview
                .url()
                .is_ok_and(|url| navigation::same_origin(&url, &SERVER_URL));
            match kind {
                PermissionKind::Camera | PermissionKind::Microphone if ours => {
                    PermissionResponse::Allow
                }
                _ => PermissionResponse::Deny,
            }
        })
        // A report export is a download, and a webview drops one unless told
        // where to put it.
        .on_download(move |webview, event| match event {
            DownloadEvent::Requested { destination, .. } => match &downloads {
                Some(dir) => {
                    let mut given = given.lock().expect("download names lock");
                    *destination = navigation::download_path(dir, destination, &given);
                    given.insert(destination.clone());
                    true
                }
                None => false,
            },
            DownloadEvent::Finished { path, success, .. } => {
                // No longer in flight, so the name is the folder's to decide
                // again: a failed download wrote nothing, and a saved file is
                // found on disk by the next one.
                if let Some(path) = &path {
                    given.lock().expect("download names lock").remove(path);
                }
                if success {
                    let saved = path
                        .map(|path| format!("Saved to {}.", path.display()))
                        .unwrap_or_else(|| "Saved to your Downloads folder.".to_string());
                    webview
                        .app_handle()
                        .dialog()
                        .message(saved)
                        .title("Download")
                        .show(|_| {});
                }
                true
            }
            _ => true,
        })
        .build()?;

    let _ = app.state::<Shell>().start_page.set(window.url()?);

    #[cfg(target_os = "linux")]
    window.with_webview(|webview| {
        use webkit2gtk::{SettingsExt, WebViewExt};
        if let Some(settings) = WebViewExt::settings(&webview.inner()) {
            settings.set_enable_media_stream(true);
            settings.set_enable_webrtc(true);
        }
    })?;
    Ok(())
}

fn launch(app: &AppHandle) {
    let prepared = (|| -> Result<server::Launch, String> {
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("cannot find a folder for CodeTrial's data: {error}"))?;
        std::fs::create_dir_all(&data_dir)
            .map_err(|error| format!("cannot create {}: {error}", data_dir.display()))?;
        Ok(server::Launch {
            program: server::program_path()?,
            data_dir,
            addr: SERVER_ADDR.to_string(),
            import_config: std::env::current_dir()
                .ok()
                .and_then(|dir| server::cli_config_near(&dir)),
        })
    })();
    let launch = match prepared {
        Ok(launch) => launch,
        Err(message) => return show_failure(app, &message, None),
    };

    let data_dir = launch.data_dir.clone();
    let reporter = app.clone();
    let started = server::start(&launch, move |status| {
        let app = &reporter;
        match status {
            server::Status::Ready => {
                let shell = app.state::<Shell>();
                if let Phase::Starting { server_ready, .. } =
                    &mut *shell.phase.lock().expect("phase lock")
                {
                    *server_ready = true;
                }
                open_server(app);
            }
            server::Status::Exited(message) => {
                if !app.state::<Shell>().quitting() {
                    show_failure(app, &message, Some(data_dir.as_path()));
                }
            }
        }
    });
    match started {
        Ok(server) => {
            *app.state::<Shell>().server.lock().expect("server lock") = Some(server);
        }
        Err(message) => show_failure(app, &message, Some(launch.data_dir.as_path())),
    }
}

/// Shows the server in this window, or in the default browser when this
/// window has no WebRTC, once both halves are known: the server is ready and
/// the starting page has said what it has. Called as each arrives, in either
/// order.
///
/// The interview is a LiveKit call, which is WebRTC, and WebKitGTK as several
/// Linux distributions build it has none: the page would load, take the camera
/// and then fail to connect. The browser is the route that works there, and
/// this process still starts and stops the server it is pointed at.
///
/// The starting page reports by navigating rather than being asked, because a
/// script sent to a page still loading is lost with the document it ran in,
/// and the server is often ready before the page is.
fn open_server(app: &AppHandle) {
    let shell = app.state::<Shell>();
    let Some(window) = app.get_webview_window(WINDOW) else {
        return;
    };

    // Decided and recorded under the lock, acted on after it is released: the
    // navigation below reaches `on_navigation`, which takes the same lock.
    let webrtc = {
        let mut phase = shell.phase.lock().expect("phase lock");
        let Phase::Starting {
            server_ready: true,
            webrtc: Some(webrtc),
        } = *phase
        else {
            return;
        };
        *phase = if webrtc {
            Phase::Serving
        } else {
            Phase::InBrowser
        };
        webrtc
    };
    if webrtc {
        note("the web view has WebRTC: the interview runs in this window");
        let _ = window.navigate(SERVER_URL.clone());
        return;
    }
    note("the web view has no WebRTC: the interview runs in the default browser");
    open_in_browser(SERVER_URL.as_str());
    show_start_page(app, &[("browser", "")]);
}

/// Errors are ignored: there is no better place to report a browser that will
/// not open than the window the person is already looking at, and the starting
/// page there offers to try again.
fn open_in_browser(url: &str) {
    let _ = tauri_plugin_opener::open_url(url, None::<&str>);
}

/// The starting page again, with `pairs` as its whole query.
fn show_start_page(app: &AppHandle, pairs: &[(&str, &str)]) {
    let shell = app.state::<Shell>();
    let (Some(page), Some(window)) = (shell.start_page.get(), app.get_webview_window(WINDOW))
    else {
        return;
    };
    let mut page = page.clone();
    page.query_pairs_mut().clear().extend_pairs(pairs);
    let _ = window.navigate(page);
}

/// For whoever started the app from a terminal. Ignores a stderr that is
/// gone, which a release build on Windows has from the start.
fn note(message: &str) {
    let _ = writeln!(std::io::stderr(), "codetrial-desktop: {message}");
}

/// Takes the window off the server's port for good, onto the starting page
/// with the reason. Never back to the port: whatever answers there now is not
/// a server this process started.
fn show_failure(app: &AppHandle, message: &str, data_dir: Option<&Path>) {
    *app.state::<Shell>().phase.lock().expect("phase lock") = Phase::Failed;
    let logs = data_dir.map(|dir| dir.display().to_string());
    let mut pairs = vec![("error", message)];
    if let Some(logs) = &logs {
        pairs.push(("logs", logs));
    }
    show_start_page(app, &pairs);
}

/// One close at a time, whichever way it was asked for: the window's close
/// button, Cmd+Q, or the dock. A second request while one is being decided
/// joins it rather than starting another dialog.
fn request_quit(app: &AppHandle) {
    let shell = app.state::<Shell>();
    {
        let mut close = shell.close.lock().expect("close lock");
        if *close != Close::Open {
            return;
        }
        *close = Close::Asking;
    }
    let phase = *shell.phase.lock().expect("phase lock");
    if phase == Phase::InBrowser {
        return confirm_quit(
            app,
            "CodeTrial is running in your web browser. Quitting stops it, and an \
             interview in progress there ends without its report.",
        );
    }
    let Some(window) = app
        .get_webview_window(WINDOW)
        .filter(|_| phase == Phase::Serving)
    else {
        return quit(app);
    };

    // The sender lives in the callback, so an eval that could not be sent drops
    // it and the wait below ends at once rather than at the timeout.
    let (answer, answered) = mpsc::channel();
    let _ = window.eval_with_callback(CLOSE_PROBE, move |result| {
        let _ = answer.send(result);
    });
    let app = app.clone();
    std::thread::spawn(move || match answered.recv_timeout(PAGE_ANSWER) {
        Ok(result) if result.contains("free") => quit(&app),
        Ok(_) => confirm_quit(
            &app,
            "An interview is in progress and its report is not saved yet. If you \
             quit now, the interview ends and the report is lost.",
        ),
        Err(_) => confirm_quit(
            &app,
            "CodeTrial is not responding, so it cannot say whether an interview is \
             in progress. Quit anyway?",
        ),
    });
}

fn confirm_quit(app: &AppHandle, message: &str) {
    let answer = app.clone();
    app.dialog()
        .message(message)
        .title("Quit CodeTrial?")
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Quit".to_string(),
            "Keep CodeTrial open".to_string(),
        ))
        .show(move |quit_now| {
            if quit_now {
                quit(&answer);
            } else {
                *answer.state::<Shell>().close.lock().expect("close lock") = Close::Open;
            }
        });
}

/// Stops the server, then exits. Off the event loop, because the stop waits
/// for the server and the window has to keep drawing while it does.
fn quit(app: &AppHandle) {
    let shell = app.state::<Shell>();
    *shell.close.lock().expect("close lock") = Close::Quitting;
    let server = shell.server.lock().expect("server lock").take();
    let app = app.clone();
    std::thread::spawn(move || {
        if let Some(server) = server {
            server.stop(STOP_GRACE);
        }
        app.exit(0);
    });
}

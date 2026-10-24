//! The `codetrial web` child process: where it lives, how it is started, when
//! it is ready, and how it is stopped.
//!
//! Nothing here touches the window, so all of it can be tested without one.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the server prints once its socket is bound, for Setup and for the full
/// app alike. Enough to load the page: the socket is listening from the bind,
/// so a connection made before the server accepts waits in the backlog rather
/// than being refused, and the Setup socket is handed over to the full app
/// rather than closed.
const BOUND_LINE: &str = "codetrial: open http://";

/// How much of the server's stderr explains an exit. The reason is its last
/// line; the ones before it are the warnings it printed on the way.
const REASON_LINES: usize = 12;

const SERVER_LOG: &str = "codetrial-server.log";

/// Where the CLI keeps its config, relative to the directory it starts in:
/// `config/` in a checkout first, then a bare file beside it.
const CLI_CONFIGS: [&str; 2] = ["config/codetrial.env.local", "codetrial.env.local"];

/// Everything one launch of the server needs, decided by the caller.
pub struct Launch {
    pub program: PathBuf,
    pub data_dir: PathBuf,
    /// `127.0.0.1:PORT`. Also the only `Host` the server will answer, so it has
    /// to be the exact spelling the window loads.
    pub addr: String,
    /// A CLI config whose keys Setup offers to use on a first launch. The
    /// server decides what is offered and what using it writes.
    pub import_config: Option<PathBuf>,
}

/// The CLI config nearest `start`: the CLI's own two places in `start`, then in
/// each folder above it. Upward because a checkout's development run starts in
/// `desktop/`, one level below the `config/` the CLI wrote; an app opened from
/// the Dock, Finder or the Start menu starts where nothing is found, and Setup
/// asks as it always has.
pub fn cli_config_near(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .flat_map(|dir| CLI_CONFIGS.map(|name| dir.join(name)))
        .find(|path| path.is_file())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    /// Listening on `Launch::addr`.
    Ready,
    /// Gone, and why, in words for the person looking at the window.
    Exited(String),
}

pub struct Server {
    child: Arc<Mutex<Child>>,
    stdin: Option<ChildStdin>,
    stopping: Arc<AtomicBool>,
}

/// The server program shipped beside this one, or the one
/// `CODETRIAL_DESKTOP_SERVER` names for a development build that has no
/// bundle to sit in.
pub fn program_path() -> Result<PathBuf, String> {
    let path = match std::env::var_os("CODETRIAL_DESKTOP_SERVER") {
        Some(path) => PathBuf::from(path),
        None => std::env::current_exe()
            .map_err(|error| format!("cannot locate this program: {error}"))?
            .with_file_name(if cfg!(windows) {
                "codetrial.exe"
            } else {
                "codetrial"
            }),
    };
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "The CodeTrial server program is missing: {}. Reinstall CodeTrial.",
            path.display()
        ))
    }
}

/// The server, told everything a desktop install needs as flags. A
/// `--data-dir` launch reads no configuration from the environment and makes
/// its own session key in that folder, so neither is decided here.
fn command(launch: &Launch) -> Command {
    let mut command = Command::new(&launch.program);
    command
        .arg("web")
        .arg("--data-dir")
        .arg(&launch.data_dir)
        .arg("--web-addr")
        .arg(&launch.addr)
        .arg("--allowed-hosts")
        .arg(&launch.addr);
    if let Some(path) = &launch.import_config {
        command.arg("--import-config").arg(path);
    }
    command
        // Not for finding the config, which `--data-dir` already decides, but
        // for a relative path written into it, such as `CODETRIAL_DB_PATH`: the
        // data folder is the one place it can sensibly mean.
        .current_dir(&launch.data_dir)
        .env("CODETRIAL_EXIT_WITH_STDIN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // The server is a console program, and a window-subsystem parent starting
    // one gets a console window beside it unless told otherwise.
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    }
    command
}

/// Starts the server and reports to `report` once: `Ready` and later perhaps
/// `Exited`, or `Exited` alone. Never `Ready` after `Exited`, so a window that
/// has been told the server is gone is not sent back to the port.
pub fn start(
    launch: &Launch,
    report: impl Fn(Status) + Send + Sync + 'static,
) -> Result<Server, String> {
    let log_path = launch.data_dir.join(SERVER_LOG);
    let log = Arc::new(Mutex::new(File::create(&log_path).map_err(|error| {
        format!("cannot write {}: {error}", log_path.display())
    })?));

    let mut child = command(launch).spawn().map_err(|error| {
        format!(
            "cannot start the CodeTrial server {}: {error}",
            launch.program.display()
        )
    })?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let child = Arc::new(Mutex::new(child));
    let stopping = Arc::new(AtomicBool::new(false));

    // One lock decides the order the two reports can arrive in.
    let finished = Arc::new(Mutex::new(false));
    let report = Arc::new(report);
    let reason = Arc::new(Mutex::new(VecDeque::with_capacity(REASON_LINES)));

    let mut copiers = Vec::new();
    if let Some(stderr) = stderr {
        let (log, reason) = (log.clone(), reason.clone());
        copiers.push(std::thread::spawn(move || {
            copy_lines(stderr, &log, |line| {
                let mut reason = reason.lock().expect("reason lock");
                if reason.len() == REASON_LINES {
                    reason.pop_front();
                }
                reason.push_back(line.to_string());
            });
        }));
    }
    if let Some(stdout) = stdout {
        let (log, finished, report) = (log.clone(), finished.clone(), report.clone());
        copiers.push(std::thread::spawn(move || {
            let mut bound = false;
            copy_lines(stdout, &log, |line| {
                if bound || !line.starts_with(BOUND_LINE) {
                    return;
                }
                bound = true;

                // Held across the report: a guard taken in the `if` alone is
                // dropped before its body runs, and `Exited` could land
                // between.
                let finished = finished.lock().expect("report order lock");
                if !*finished {
                    report(Status::Ready);
                }
            });
        }));
    }

    {
        let (child, stopping) = (child.clone(), stopping.clone());
        std::thread::spawn(move || {
            let status = loop {
                if let Ok(Some(status)) = child.lock().expect("child lock").try_wait() {
                    break status;
                }
                std::thread::sleep(Duration::from_millis(100));
            };

            // The reason is the last lines the server wrote, so they have to be
            // read first. Bounded, because a pipe stays open for as long as
            // anything the server started still holds it.
            let deadline = Instant::now() + Duration::from_secs(1);
            while copiers.iter().any(|copier| !copier.is_finished()) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let mut finished = finished.lock().expect("report order lock");
            *finished = true;
            if !stopping.load(Ordering::SeqCst) {
                let reason = reason.lock().expect("reason lock");
                report(Status::Exited(exit_message(
                    &status.to_string(),
                    reason.iter().map(String::as_str),
                )));
            }
        });
    }

    Ok(Server {
        child,
        stdin,
        stopping,
    })
}

impl Server {
    /// Closes the server's stdin, which it exits on, and kills it if it has
    /// not gone within `grace`.
    pub fn stop(mut self, grace: Duration) {
        self.stopping.store(true, Ordering::SeqCst);
        drop(self.stdin.take());
        let deadline = Instant::now() + grace;
        loop {
            let mut child = self.child.lock().expect("child lock");
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            drop(child);
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Lossy rather than `lines()`, which ends at the first line that is not
/// UTF-8: the pipe would then go unread, and a server writing to it blocks once
/// it fills, or dies on the broken pipe once the reader is dropped.
fn copy_lines(stream: impl Read, log: &Mutex<File>, mut each: impl FnMut(&str)) {
    let mut reader = BufReader::new(stream);
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&bytes);
        let line = text.trim_end_matches(['\n', '\r']);
        let _ = writeln!(log.lock().expect("log lock"), "{line}");
        each(line);
    }
}

/// Why the server exited, from the last lines it wrote to stderr, which end
/// with the reason when it gave one.
fn exit_message<'a>(status: &str, reason: impl Iterator<Item = &'a str>) -> String {
    let reason = reason.collect::<Vec<_>>().join("\n");
    let reason = if reason.trim().is_empty() {
        "It printed nothing."
    } else {
        &reason
    };
    format!("The CodeTrial server stopped ({status}).\n\n{reason}")
}

#[cfg(test)]
#[path = "../tests/unit/server.rs"]
mod tests;

use std::path::Path;
use std::sync::mpsc;

use super::*;

const CODE: &str = "0123456789abcdef0123456789abcdef";

fn temp_dir(label: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "codetrial-desktop-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_server_is_started_on_the_data_dir_and_told_the_rest() {
    let launch = Launch {
        program: PathBuf::from("codetrial"),
        data_dir: PathBuf::from("/data"),
        addr: "127.0.0.1:3101".to_string(),
        import_config: None,
        change_keys_code: CODE.to_string(),
    };
    let command = command(&launch);
    let args = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        args,
        [
            "web",
            "--data-dir",
            "/data",
            "--web-addr",
            "127.0.0.1:3101",
            "--allowed-hosts",
            "127.0.0.1:3101",
            "--no-update-check"
        ]
    );
    assert_eq!(command.get_current_dir(), Some(Path::new("/data")));

    // The pipe, the Change keys code and that this app restarts the server when
    // asked to are what it is told through the environment, which a
    // `--data-dir` launch reads no configuration from.
    let env = |key: &str| {
        command
            .get_envs()
            .find(|(name, _)| *name == key)
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
    };
    assert_eq!(command.get_envs().count(), 3);
    assert_eq!(env("CODETRIAL_EXIT_WITH_STDIN").as_deref(), Some("1"));
    assert_eq!(env("CODETRIAL_CHANGE_KEYS_CODE").as_deref(), Some(CODE));
    assert_eq!(env("CODETRIAL_RESTART_AFTER_SAVE").as_deref(), Some("1"));
}

#[test]
fn a_cli_config_found_is_offered_to_the_server_to_import() {
    let launch = Launch {
        program: PathBuf::from("codetrial"),
        data_dir: PathBuf::from("/data"),
        addr: "127.0.0.1:3101".to_string(),
        import_config: Some(PathBuf::from("/checkout/config/codetrial.env.local")),
        change_keys_code: CODE.to_string(),
    };
    let args = command(&launch)
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        args[args.len() - 2..],
        ["--import-config", "/checkout/config/codetrial.env.local"]
    );
}

/// Only the start folder is read, and in it `config/` comes before a bare file,
/// as the CLI searches them.
#[test]
fn the_cli_config_is_looked_for_in_the_start_folder_and_nowhere_else() {
    let root = temp_dir("cli-config");
    let start = root.join("checkout").join("desktop");
    std::fs::create_dir_all(&start).unwrap();
    let above = root.join("checkout").join("codetrial.env.local");
    let above_config = root
        .join("checkout")
        .join("config")
        .join("codetrial.env.local");
    std::fs::create_dir_all(above_config.parent().unwrap()).unwrap();
    std::fs::write(&above, "").unwrap();
    std::fs::write(&above_config, "").unwrap();
    // A file in a folder above is another project's as far as the app knows.
    assert_eq!(cli_config_in(&start), None);

    let bare = start.join("codetrial.env.local");
    let in_config = start.join("config").join("codetrial.env.local");
    std::fs::create_dir_all(in_config.parent().unwrap()).unwrap();
    std::fs::write(&bare, "").unwrap();
    assert_eq!(cli_config_in(&start).as_deref(), Some(bare.as_path()));
    std::fs::write(&in_config, "").unwrap();
    assert_eq!(cli_config_in(&start).as_deref(), Some(in_config.as_path()));
    // A folder of that name is not a config.
    std::fs::remove_file(&in_config).unwrap();
    std::fs::create_dir_all(&in_config).unwrap();
    assert_eq!(cli_config_in(&start).as_deref(), Some(bare.as_path()));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_exit_is_explained_by_what_the_server_last_wrote() {
    assert!(exit_message("exit status: 1", std::iter::empty()).ends_with("It printed nothing."));

    let message = exit_message(
        "exit status: 1",
        [
            "a warning on the way",
            "failed to bind 127.0.0.1:3101: in use",
        ]
        .into_iter(),
    );
    assert!(message.contains("(exit status: 1)"), "{message}");
    assert!(
        message.ends_with("a warning on the way\nfailed to bind 127.0.0.1:3101: in use"),
        "{message}"
    );
}

/// Held from writing a stand-in server until it has started. A spawn on
/// another test's thread forks a child that keeps every descriptor open at
/// that moment until it execs, so a script still open for writing then cannot
/// be executed: "Text file busy". The rule this sets: every process this test
/// binary starts, a `kill` or any other helper included, is started under
/// `SPAWNING`, and a stand-in server through `start_fake`.
#[cfg(unix)]
static SPAWNING: Mutex<()> = Mutex::new(());

/// Starts a stand-in server: a shell script, so the process handling is what
/// is tested and not the CodeTrial server.
#[cfg(unix)]
fn start_fake(dir: &Path, body: &str, report: impl Fn(Status) + Send + Sync + 'static) -> Server {
    use std::os::unix::fs::PermissionsExt;
    // A test that failed while holding the lock left nothing half done.
    let _spawning = SPAWNING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let program = dir.join("fake-codetrial");
    std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let launch = Launch {
        program,
        data_dir: dir.to_path_buf(),
        addr: "127.0.0.1:9".to_string(),
        import_config: None,
        change_keys_code: CODE.to_string(),
    };
    start(&launch, report).unwrap()
}

#[cfg(unix)]
#[test]
fn a_server_that_dies_at_start_is_reported_with_what_it_said() {
    let dir = temp_dir("dies");
    let (sender, received) = mpsc::channel();
    let sender = Mutex::new(sender);
    let _server = start_fake(
        &dir,
        "echo starting; echo 'it went wrong' >&2; exit 3",
        move |status| {
            let _ = sender.lock().unwrap().send(status);
        },
    );

    let Status::Exited(message) = received.recv_timeout(Duration::from_secs(10)).unwrap() else {
        panic!("a server that never bound cannot be ready");
    };
    assert!(message.contains("exit status: 3"), "{message}");
    assert!(message.contains("it went wrong"), "{message}");
    assert!(received.recv_timeout(Duration::from_millis(300)).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ready on the line the server prints once it is listening, and stopped by
/// its stdin closing. The script waits on its stdin exactly as the server does
/// with `CODETRIAL_EXIT_WITH_STDIN`, so a stop that forgot to close the pipe
/// would fall through to the kill and leave the marker unwritten.
#[cfg(unix)]
#[test]
fn a_server_is_ready_once_bound_and_stops_when_its_stdin_closes() {
    let dir = temp_dir("ready");
    let marker = dir.join("stdin-closed");
    let (sender, received) = mpsc::channel();
    let sender = Mutex::new(sender);
    let server = start_fake(
        &dir,
        &format!(
            "echo 'codetrial 0.1.0 (abc1234)'\n\
             echo 'codetrial: open http://127.0.0.1:9 in your browser'\n\
             cat >/dev/null\ntouch '{}'",
            marker.display()
        ),
        move |status| {
            let _ = sender.lock().unwrap().send(status);
        },
    );

    assert_eq!(
        received.recv_timeout(Duration::from_secs(10)).unwrap(),
        Status::Ready
    );
    assert!(
        received.recv_timeout(Duration::from_millis(300)).is_err(),
        "one line, one report"
    );

    server.stop(Duration::from_secs(5));
    assert!(
        marker.exists(),
        "the server should have seen its stdin close"
    );

    // An exit the app asked for is not news to report.
    assert!(received.recv_timeout(Duration::from_millis(500)).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A restart asked for after the server is ready, and only then: the same line
/// before it is not one this server printed.
#[cfg(unix)]
#[test]
fn a_server_asks_to_restart_once_change_keys_has_saved() {
    let dir = temp_dir("restart");
    let (sender, received) = mpsc::channel();
    let sender = Mutex::new(sender);
    let server = start_fake(
        &dir,
        "echo 'codetrial: keys saved and no interview running: restart now'\n\
         echo 'codetrial: open http://127.0.0.1:9 in your browser'\n\
         echo 'codetrial: keys saved and no interview running: restart now'\n\
         cat >/dev/null",
        move |status| {
            let _ = sender.lock().unwrap().send(status);
        },
    );

    let wait = Duration::from_secs(10);
    assert_eq!(received.recv_timeout(wait).unwrap(), Status::Ready);
    assert_eq!(received.recv_timeout(wait).unwrap(), Status::Restart);
    assert!(received.recv_timeout(Duration::from_millis(300)).is_err());

    server.stop(Duration::from_secs(5));
    assert!(received.recv_timeout(Duration::from_millis(500)).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A restart appends to the server log rather than emptying it. The old server
/// leaves a child holding its stdout, so a line arrives after it has stopped
/// and the next one has started: emptied, the log would take that line past
/// its end, after a run of NULs, and lose what the old run printed.
#[cfg(unix)]
#[test]
fn a_restart_keeps_the_log_of_the_run_it_ends() {
    let dir = temp_dir("log-restart");
    let log = dir.join(SERVER_LOG);
    let read = || std::fs::read(&log).unwrap();
    clear_log(&dir).unwrap();
    let old = start_fake(
        &dir,
        "echo 'first run: LiveKit rejected the key'\n\
         (sleep 1; echo 'first run: written after the restart') &\n\
         cat >/dev/null",
        |_| {},
    );
    let began = Instant::now();
    while !String::from_utf8_lossy(&read()).contains("rejected") {
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "the old run wrote nothing"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    old.stop(Duration::from_secs(5));
    log_line(&dir, "codetrial-desktop: restarted");
    let next = start_fake(&dir, "echo 'second run'\ncat >/dev/null", |_| {});
    let began = Instant::now();
    while !String::from_utf8_lossy(&read()).contains("after the restart") {
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "the late line never came"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    next.stop(Duration::from_secs(5));

    let bytes = read();
    assert!(!bytes.contains(&0), "{:?}", String::from_utf8_lossy(&bytes));
    let text = String::from_utf8(bytes).unwrap();
    let at = |line: &str| {
        text.find(line)
            .unwrap_or_else(|| panic!("{line:?} in {text:?}"))
    };
    assert!(at("first run: LiveKit rejected the key") < at("codetrial-desktop: restarted"));
    assert!(at("codetrial-desktop: restarted") < at("second run"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The lines this app waits for are the server's own words, and the desktop
/// crate cannot share the server's constants. The fake servers here print this
/// crate's copies, so without this a reworded line on either side keeps every
/// test green while the app stops hearing it: a missed restart line leaves a
/// server that admits nothing until the app quits.
#[test]
fn the_lines_waited_for_are_the_ones_the_server_prints() {
    let setup_edit = include_str!("../../../src/web/setup_edit.rs");
    assert!(setup_edit.contains(&format!("\"{RESTART_LINE}\"")));
    let launch = include_str!("../../../src/launch.rs");
    assert!(launch.contains(&format!("\"{BOUND_LINE}{{bound}}")));
}

/// Opening the app empties what the last run left in the server log, and makes
/// the file when there is none, so a restart has something to append to.
#[test]
fn opening_the_app_empties_the_log_of_the_last_run() {
    let dir = temp_dir("log-clear");
    let log = dir.join(SERVER_LOG);
    clear_log(&dir).unwrap();
    assert_eq!(std::fs::read(&log).unwrap(), b"");

    std::fs::write(&log, "left by the last run\n").unwrap();
    clear_log(&dir).unwrap();
    assert_eq!(std::fs::read(&log).unwrap(), b"");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A fake server started in a folder of its own under `dir`, with the process
/// id it wrote there once running.
#[cfg(unix)]
fn running(dir: &Path, name: &str, body: &str) -> (Server, String) {
    let dir = dir.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let pid = dir.join("pid");

    // Written to a name of its own and then moved into place, so the file never
    // exists empty: `>` creates it before it writes the id, and a read in
    // between would take no id for a server that is not running.
    let server = start_fake(
        &dir,
        &format!(
            "echo $$ > '{pid}.tmp' && mv '{pid}.tmp' '{pid}'
{body}",
            pid = pid.display()
        ),
        |_| {},
    );
    let began = Instant::now();
    while !pid.exists() && began.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let id = std::fs::read_to_string(pid).unwrap().trim().to_string();
    assert!(
        id.parse::<u32>().is_ok(),
        "no process id was written: {id:?}"
    );
    (server, id)
}

/// What a server reports is acted on only once it is installed in its slot: a
/// restart asked for in the gap between `start` returning and `put` would find
/// nothing to stop and start a second server beside the first.
#[cfg(unix)]
#[test]
fn a_report_waits_until_the_server_it_came_from_is_installed() {
    let dir = temp_dir("report-after-install");
    let slot = Arc::new(Slot::default());
    let events = Arc::new(Mutex::new(Vec::new()));

    let installing = slot.installing();
    let (started_sender, started) = std::sync::mpsc::channel();
    let reporter = {
        let (slot, events) = (Arc::clone(&slot), Arc::clone(&events));
        std::thread::spawn(move || {
            started_sender.send(()).unwrap();
            slot.installed();
            events.lock().unwrap().push("reported");
        })
    };
    started.recv().unwrap();
    // Time for a report that was not held back to have gone ahead.
    std::thread::sleep(Duration::from_millis(200));

    let (server, _) = running(&dir, "only", "cat >/dev/null");
    events.lock().unwrap().push("installed");
    slot.put(server);
    drop(installing);
    reporter.join().unwrap();

    assert_eq!(*events.lock().unwrap(), ["installed", "reported"]);
    slot.stop(Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A quit while a restart is still stopping the old server waits for that stop,
/// and for the server the restart then started, so neither outlives the app.
/// The old one ignores its stdin closing, so only the kill after the grace
/// ends it, well after the quit was asked for.
#[cfg(unix)]
#[test]
fn a_quit_during_a_restart_waits_for_both_servers_to_stop() {
    let dir = temp_dir("quit-restart");
    let alive = |pid: &str| {
        // Started under `SPAWNING`, as every process this binary starts is.
        let _spawning = SPAWNING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Command::new("kill")
            .args(["-0", pid])
            .status()
            .unwrap()
            .success()
    };
    let (old, old_pid) = running(
        &dir,
        "old",
        "cat >/dev/null
sleep 60",
    );
    let slot = Arc::new(Slot::default());
    slot.put(old);
    let next_pid = Arc::new(Mutex::new(None));
    let (next_slot, next_dir, next_seen) = (Arc::clone(&slot), dir.clone(), Arc::clone(&next_pid));
    slot.replace(Duration::from_secs(1), move || {
        let (next, pid) = running(&next_dir, "next", "cat >/dev/null");
        *next_seen.lock().unwrap() = Some(pid);
        next_slot.put(next);
    });

    slot.stop(Duration::from_secs(5));
    assert!(!alive(&old_pid), "the old server outlived the quit");
    let next_pid = next_pid.lock().unwrap().clone();
    let next_pid = next_pid.expect("the restart had not finished when the quit returned");
    assert!(
        !alive(&next_pid),
        "the server the restart started outlived the quit"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Strong enough for the server to take: it ignores a code shorter than 24
/// letters and digits.
#[test]
fn a_change_keys_code_is_new_hex_on_every_launch() {
    let first = change_keys_code().unwrap();
    assert_eq!(first.len(), 32);
    assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_ne!(first, change_keys_code().unwrap());
}

use std::path::Path;
use std::sync::mpsc;

use super::*;

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
            "127.0.0.1:3101"
        ]
    );
    assert_eq!(command.get_current_dir(), Some(Path::new("/data")));

    // The pipe is the one thing it is told through the environment, which a
    // `--data-dir` launch reads no configuration from.
    let env = |key: &str| {
        command
            .get_envs()
            .find(|(name, _)| *name == key)
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
    };
    assert_eq!(command.get_envs().count(), 1);
    assert_eq!(env("CODETRIAL_EXIT_WITH_STDIN").as_deref(), Some("1"));
}

#[test]
fn a_cli_config_found_is_offered_to_the_server_to_import() {
    let launch = Launch {
        program: PathBuf::from("codetrial"),
        data_dir: PathBuf::from("/data"),
        addr: "127.0.0.1:3101".to_string(),
        import_config: Some(PathBuf::from("/checkout/config/codetrial.env.local")),
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

/// A development run starts in `desktop/`, below the checkout's `config/`; the
/// nearest config wins, and in one folder `config/` comes before a bare file,
/// as the CLI searches them.
#[test]
fn the_cli_config_is_looked_for_from_the_start_folder_upward() {
    let root = temp_dir("cli-config");
    let start = root.join("checkout").join("desktop");
    std::fs::create_dir_all(&start).unwrap();
    let in_root = root.join("codetrial.env.local");
    let checkout_bare = root.join("checkout").join("codetrial.env.local");
    let checkout_config = root
        .join("checkout")
        .join("config")
        .join("codetrial.env.local");
    std::fs::create_dir_all(checkout_config.parent().unwrap()).unwrap();

    let found = cli_config_near(&start);
    assert!(
        !found.as_deref().is_some_and(|path| path.starts_with(&root)),
        "{found:?} was never written"
    );
    std::fs::write(&in_root, "").unwrap();
    assert_eq!(cli_config_near(&start).as_deref(), Some(in_root.as_path()));
    std::fs::write(&checkout_bare, "").unwrap();
    assert_eq!(
        cli_config_near(&start).as_deref(),
        Some(checkout_bare.as_path())
    );
    std::fs::write(&checkout_config, "").unwrap();
    assert_eq!(
        cli_config_near(&start).as_deref(),
        Some(checkout_config.as_path())
    );
    // A folder of that name is not a config.
    std::fs::remove_file(&checkout_config).unwrap();
    std::fs::create_dir_all(&checkout_config).unwrap();
    assert_eq!(
        cli_config_near(&start).as_deref(),
        Some(checkout_bare.as_path())
    );

    let _ = std::fs::remove_dir_all(&root);
}

// The app waits for this text and nothing else, and it lives in another crate,
// so a reworded message would leave the window on the starting page forever.
// Both lines count: Setup prints one, the full app the other.
#[test]
fn the_bound_line_is_what_the_server_prints() {
    let server = include_str!("../../../src/main.rs");
    let pattern = format!("println!(\"{BOUND_LINE}{{bound}} in your browser");
    assert_eq!(server.matches(&pattern).count(), 2, "{pattern}");
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

/// A stand-in server: a shell script, so the process handling is what is
/// tested and not the CodeTrial server.
#[cfg(unix)]
fn fake_server(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let program = dir.join("fake-codetrial");
    std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    program
}

#[cfg(unix)]
fn launch_of(dir: &Path, program: PathBuf, addr: String) -> Launch {
    Launch {
        program,
        data_dir: dir.to_path_buf(),
        addr,
        import_config: None,
    }
}

#[cfg(unix)]
#[test]
fn a_server_that_dies_at_start_is_reported_with_what_it_said() {
    let dir = temp_dir("dies");
    let program = fake_server(&dir, "echo starting; echo 'it went wrong' >&2; exit 3");
    let (sender, received) = mpsc::channel();
    let sender = Mutex::new(sender);
    let _server = start(
        &launch_of(&dir, program, "127.0.0.1:9".to_string()),
        move |status| {
            let _ = sender.lock().unwrap().send(status);
        },
    )
    .unwrap();

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
    let program = fake_server(
        &dir,
        &format!(
            "echo 'codetrial 0.1.0 (abc1234)'\n\
             echo 'codetrial: open http://127.0.0.1:9 in your browser'\n\
             cat >/dev/null\ntouch '{}'",
            marker.display()
        ),
    );
    let (sender, received) = mpsc::channel();
    let sender = Mutex::new(sender);
    let server = start(
        &launch_of(&dir, program, "127.0.0.1:9".to_string()),
        move |status| {
            let _ = sender.lock().unwrap().send(status);
        },
    )
    .unwrap();

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

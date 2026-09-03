use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::accept_async;

fn run_cli_args(args: &[&str]) -> (i32, String, String) {
    run_cli_args_with_env(args, &[])
}

fn run_cli_args_with_env(args: &[&str], envs: &[(&str, &str)]) -> (i32, String, String) {
    let cwd = exe_temp_path("empty-cwd");
    std::fs::create_dir_all(&cwd).expect("empty cwd should create");
    let output = cli_command(args, envs, &cwd)
        .output()
        .expect("codetrial should exit");
    let _ = std::fs::remove_dir_all(cwd);

    (
        output.status.code().unwrap_or_default(),
        String::from_utf8(output.stdout).expect("stdout should be UTF-8"),
        String::from_utf8(output.stderr).expect("stderr should be UTF-8"),
    )
}

/// The same invocation, but the process is required to stop by itself.
///
/// `run_cli_args` waits forever, which is right for a mode that always exits.
/// A refusal test is not that: what it asserts is that the binary *stops*, so a
/// regression letting it serve instead hangs the suite rather than failing it,
/// and `cargo mutants` scores that as a timeout instead of a caught mutant.
/// That is not hypothetical; it is how `replace == with != in run_web` reached
/// CI as a 33-second timeout. Bounded, so a guard that stopped guarding is a
/// red test.
fn run_cli_until_exit(args: &[&str]) -> (i32, String, String) {
    // Generous for a loaded runner, and well inside the timeout `cargo mutants`
    // derives from the baseline (33s when this was written). A bound above that
    // would score a caught mutant as a timeout again, which is the failure this
    // whole helper exists to remove.
    const LIMIT: Duration = Duration::from_secs(10);

    let cwd = exe_temp_path("empty-cwd");
    std::fs::create_dir_all(&cwd).expect("empty cwd should create");
    let mut child = cli_command(args, &[], &cwd)
        .spawn()
        .expect("codetrial should start");

    // Drained while the child runs, not after it exits. A pipe holds about
    // 64KB; a child that filled one would block in `write` and never reach the
    // exit this loop is watching for, so the helper would report a refusal that
    // did not happen as a timeout that did not either. These paths print a few
    // lines, but the trap belongs to whoever reuses this next.
    let mut out = child.stdout.take().expect("stdout is piped");
    let mut err = child.stderr.take().expect("stderr is piped");
    let out = thread::spawn(move || {
        let mut text = String::new();
        out.read_to_string(&mut text)
            .expect("stdout should be UTF-8");
        text
    });
    let err = thread::spawn(move || {
        let mut text = String::new();
        err.read_to_string(&mut text)
            .expect("stderr should be UTF-8");
        text
    });

    let deadline = Instant::now() + LIMIT;
    let status = loop {
        match child.try_wait().expect("child status should be readable") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                // Kills and reaps, which also closes the pipes and lets the two
                // readers finish rather than parking this thread on `join`.
                stop_child(&mut child);
                let _ = std::fs::remove_dir_all(&cwd);
                panic!("codetrial {args:?} was still running after {LIMIT:?}; it had to refuse");
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    };

    let stdout = out.join().expect("stdout reader should finish");
    let stderr = err.join().expect("stderr reader should finish");
    let _ = std::fs::remove_dir_all(&cwd);

    (status.code().unwrap_or_default(), stdout, stderr)
}

/// One place builds the invocation, so the two runners cannot drift about which
/// environment the child inherits. The removals matter: a developer with
/// `NODE_ENV` or `SESSION_SECRET` exported would otherwise change what these
/// tests are testing.
/// Runs a linked binary inside `cwd`, not the one in the target directory:
/// the executable's own folder is a config search path and a place the Setup
/// page writes, so a stray `codetrial.env.local` next to the real test binary
/// would decide these tests.
fn cli_command(args: &[&str], envs: &[(&str, &str)], cwd: &Path) -> Command {
    let mut command = Command::new(binary_beside(cwd));
    command
        .args(args)
        .current_dir(cwd)
        .env_remove("LIVEKIT_URL")
        .env_remove("LIVEKIT_API_KEY")
        .env_remove("LIVEKIT_API_SECRET")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("CODETRIAL_ROOM_PREFIX")
        .env_remove("CODETRIAL_DURATION_MIN")
        .env_remove("CODETRIAL_WEB_DIR")
        .env_remove("CODETRIAL_WEB_ADDR")
        .env_remove("NODE_ENV")
        .env_remove("SESSION_SECRET")
        .envs(envs.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Binding then dropping is inherently racy: the child re-binds the port a
/// moment later. The OS reissuing the same freed port to a sibling test is the
/// dominant collision, so remember what has already been handed out and keep
/// each rejected listener open until a fresh port turns up.
fn free_addr() -> String {
    static TAKEN: std::sync::Mutex<Option<std::collections::HashSet<u16>>> =
        std::sync::Mutex::new(None);

    let mut taken = TAKEN.lock().expect("port registry should lock");
    let taken = taken.get_or_insert_with(std::collections::HashSet::new);
    let mut rejected = Vec::new();
    for _ in 0..64 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("free port should bind");
        let addr = listener.local_addr().unwrap();
        if taken.insert(addr.port()) {
            return addr.to_string();
        }
        rejected.push(listener);
    }
    panic!("could not find an unused local port");
}

/// A port from `free_addr` is only a hint: the listener must close before the
/// child can bind it, so an unrelated socket can take it in between. When that
/// happens the binary reports a bind failure, which would otherwise masquerade
/// as the assertion under test failing. Retry on a fresh port instead.
fn with_free_addr<T>(attempt: impl Fn(&str) -> Option<T>) -> T {
    for _ in 0..5 {
        if let Some(value) = attempt(&free_addr()) {
            return value;
        }
    }
    panic!("could not bind a free port after 5 attempts");
}

/// Unique per call, not merely per clock reading. Tests run as threads in one
/// process and two of them ask for a `cwd` at startup; a coarse clock hands
/// both the same directory, and the first one to finish deletes the other's web
/// root out from under a live server.
fn temp_path(name: &str) -> PathBuf {
    temp_path_in(std::env::temp_dir(), name)
}

/// Under the target directory rather than the system temp one, because
/// `binary_beside` hard-links the test binary in and a hard link needs both
/// ends on one filesystem.
fn exe_temp_path(name: &str) -> PathBuf {
    temp_path_in(PathBuf::from(env!("CARGO_TARGET_TMPDIR")), name)
}

/// The binary itself, linked so that `current_exe` names a path inside `dir`:
/// these tests are about the folder the executable sits in. A hard link and
/// not a copy, which would be a 300 MB debug build per test, and not a
/// symlink, which `current_exe` resolves back to the original.
fn binary_beside(dir: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "codetrial.exe"
    } else {
        "codetrial"
    };
    let exe = dir.join(name);
    std::fs::hard_link(env!("CARGO_BIN_EXE_codetrial"), &exe)
        .expect("the test binary should link into the target directory");
    exe
}

fn temp_path_in(base: PathBuf, name: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    base.join(format!(
        "codetrial-cli-{}-{nanos}-{unique}-{name}",
        std::process::id()
    ))
}

/// Waits for a served response, not merely a connectable socket. The binary
/// binds the listener before it builds the tokio runtime, so a bare connect
/// succeeds into the accept backlog while nothing is answering yet; probing
/// that way returns early and the next request can be reset.
fn wait_for_http(addr: &str, child: &mut Child) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if try_http(
            addr,
            "GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        )
        .is_some_and(|response| response.starts_with("HTTP/"))
        {
            return Ok(());
        }
        if let Some(status) = child.try_wait().expect("child status should read") {
            let mut stderr = String::new();
            if let Some(stream) = child.stderr.as_mut() {
                let _ = stream.read_to_string(&mut stderr);
            }
            return Err(format!(
                "server exited before listening: {status}; stderr={stderr}"
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err(format!("server did not answer at {addr}"))
}

/// Starts the server on a port that is only reserved until `free_addr` closes
/// its listener, so another socket can take it first. Retry on that specific
/// loss instead of reporting it as the behavior under test failing.
fn spawn_server(build: impl Fn(&str) -> Command) -> (String, ServerProcess) {
    for _ in 0..5 {
        let addr = free_addr();
        let mut child = ServerProcess::spawn(&mut build(&addr));
        match wait_for_http(&addr, &mut child) {
            Ok(()) => return (addr, child),
            Err(error) if error.contains("Address already in use") => continue,
            Err(error) => panic!("{error}"),
        }
    }
    panic!("could not start the server on a free port");
}

fn try_http(addr: &str, request: &str) -> Option<String> {
    let mut stream = TcpStream::connect(addr).ok()?;
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    Some(response)
}

fn http_request(addr: &str, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("server should accept request");
    stream
        .write_all(request.as_bytes())
        .expect("request should write");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("response should read");
    response
}

/// Kills the spawned server on drop. Without this a failing assertion unwinds
/// past `stop_child` and leaves a `codetrial web` process listening for the
/// rest of the machine's uptime; several accumulated during development.
struct ServerProcess(Child);

impl ServerProcess {
    /// stderr is piped here rather than left to each caller, because
    /// `spawn_server` decides whether to retry by reading it. A test that
    /// spawns without piping inherits stderr, so the bind failure lands on the
    /// terminal, `wait_for_http` reports `stderr=` empty, the retry sees no
    /// "Address already in use" to match on, and a port race is reported as
    /// the behavior under test failing. That is exactly what
    /// `binary_web_does_not_require_github_oauth_config` did: the one spawning
    /// test that did not pipe was the one whose retry never fired.
    fn spawn(command: &mut Command) -> Self {
        Self(
            command
                .stderr(Stdio::piped())
                .spawn()
                .expect("codetrial web should start"),
        )
    }
}

impl std::ops::Deref for ServerProcess {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for ServerProcess {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn write_config(path: &Path, web_dir: &Path, addr: &str) {
    let text = format!(
        "\
LIVEKIT_URL=wss://example.livekit.cloud
LIVEKIT_API_KEY=test-key
LIVEKIT_API_SECRET=test-secret
GOOGLE_API_KEY=test-google
CODETRIAL_ROOM_PREFIX=file
CODETRIAL_DURATION_MIN=20
CODETRIAL_WEB_DIR={}
CODETRIAL_WEB_ADDR={}
SESSION_SECRET=session-secret
CODETRIAL_DB_PATH={}/accounts.db
",
        web_dir.display(),
        addr,
        web_dir.display()
    );
    std::fs::write(path, text).expect("config should write");
}

#[test]
fn binary_livekit_runner_requires_room_name() {
    let (code, stdout, stderr) = run_cli_args(&["run-livekit"]);

    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("usage: codetrial run-livekit ROOM"));
}

#[test]
fn binary_help_exits_without_loading_config() {
    for (args, usage) in [
        (&["--help"][..], "usage: codetrial MODE [OPTIONS]"),
        (&["web", "-h"][..], "usage: codetrial web [OPTIONS]"),
        // Help beats parsing. Both of these are a flag missing its value, and
        // answering a request for help with a parse error is the wrong answer
        // to the question that was asked.
        (
            &["web", "--config", "--help"][..],
            "usage: codetrial web [OPTIONS]",
        ),
        (
            &["--help", "--config"][..],
            "usage: codetrial MODE [OPTIONS]",
        ),
    ] {
        let (code, stdout, stderr) = run_cli_args(args);

        assert_eq!(code, 0, "{args:?}");
        assert!(stdout.contains(usage), "{args:?}: {stdout}");
        assert!(stderr.is_empty(), "{args:?}: {stderr}");

        // The options, not just the usage line. Help that names the modes and
        // stops is help that does not answer "how do I point it at my config".
        for flag in ["--config PATH", "--web-addr ADDR", "-h, --help"] {
            assert!(
                stdout.contains(flag),
                "{args:?} must document {flag}: {stdout}"
            );
        }
        assert!(
            stdout.contains("--flag=value"),
            "{args:?} must say how to pass a dash-prefixed value: {stdout}"
        );
    }
}

/// `--flag=value` is the only way to pass a value that starts with a dash, and
/// `--` is the only way to pass a positional that does. Without both, the guard
/// that stops `--config --help` from naming a file `--help` also makes a real
/// path like `-dashfile.env` unreachable.
#[test]
fn binary_accepts_dash_prefixed_values_through_the_attached_form() {
    let dir = temp_path("dash-values");
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("-dashfile.env");
    write_config(&config, &dir, "127.0.0.1:1");
    let (code, _, stderr) = run_cli_args(&[
        "web",
        &format!("--config={}", config.to_str().unwrap()),
        "--web-addr=127.0.0.1:1",
    ]);
    let _ = std::fs::remove_dir_all(dir);

    // It read the file: the only complaint left is the port it cannot bind.
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("failed to bind"), "{stderr}");
    assert!(
        !stderr.contains("required configuration file is missing"),
        "the attached form must reach a dash-prefixed path: {stderr}"
    );
}

/// A flag in the value slot is a missing value, not the value. Without that,
/// `--config` followed by another flag reports a configuration file named
/// `--web-addr`, and the operator goes looking for a path that never existed.
#[test]
fn binary_rejects_a_flag_standing_in_for_a_flag_value() {
    let (code, stdout, stderr) = run_cli_args(&["web", "--config", "--web-addr"]);

    assert_eq!(code, 2);
    assert!(stdout.is_empty());
    assert!(stderr.contains("--config requires a value"), "{stderr}");
}

/// The message names the file that was read, not a fixed `config/` path. A
/// server started with `--config` elsewhere would otherwise send whoever runs
/// it to edit a file it never opened.
/// `--` ends the options, so what follows is a positional whatever it looks
/// like, and the separator itself is not one of them.
///
/// `run-livekit` takes a room name, which is what makes the count observable
/// from outside: exactly one positional after the mode word gets past the arity
/// check and on to loading a config, and a separator left in the list, or a
/// value dropped from it, stops there with a usage line instead.
#[test]
fn binary_treats_everything_after_a_double_dash_as_positional() {
    let (code, stdout, stderr) = run_cli_args(&["run-livekit", "--", "-interview-dash"]);

    assert_eq!(
        code, 1,
        "the room name must have been the only positional: {stderr}"
    );
    assert!(stdout.is_empty());
    assert!(
        stderr.contains("required configuration file is missing"),
        "it got past the arity check and on to the config: {stderr}"
    );
    assert!(!stderr.contains("unknown flag"), "{stderr}");

    // `web` takes no positional at all, so the same separator is refused there.
    // Together the two pin the count rather than just the parse.
    let (code, _, stderr) = run_cli_args(&["web", "--", "--web-addr"]);
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("usage: codetrial web"), "{stderr}");
    assert!(!stderr.contains("unknown flag"), "{stderr}");
}

#[test]
fn binary_web_names_the_config_it_read_when_credentials_are_missing() {
    let dir = temp_path("credential-less");
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("codetrial.env.local");
    std::fs::write(&config, "CODETRIAL_WEB_ADDR=127.0.0.1:1\n").unwrap();
    let (code, stdout, stderr) = run_cli_args(&["web", "--config", config.to_str().unwrap()]);
    let _ = std::fs::remove_dir_all(dir);

    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(
        stderr.contains("missing required LiveKit credentials"),
        "{stderr}"
    );
    assert!(stderr.contains(config.to_str().unwrap()), "{stderr}");
}

/// A release binary is unpacked into a folder of its own and keeps its config
/// there, so the file has to be found from a shortcut or a terminal opened
/// somewhere else and not only from a double-click, where the working
/// directory happens to be the same folder.
#[test]
fn binary_reads_a_config_file_beside_the_executable() {
    let dir = exe_temp_path("config-beside-exe");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = binary_beside(&dir);
    let config = dir.join("codetrial.env.local");
    std::fs::write(&config, "CODETRIAL_WEB_ADDR=127.0.0.1:1\n").unwrap();

    // An empty working directory, so nothing but the executable's own folder
    // can be what answered.
    let cwd = temp_path("elsewhere");
    std::fs::create_dir_all(&cwd).unwrap();
    let output = Command::new(&exe)
        .arg("web")
        .current_dir(&cwd)
        .env_remove("LIVEKIT_URL")
        .env_remove("LIVEKIT_API_KEY")
        .env_remove("LIVEKIT_API_SECRET")
        .env_remove("GOOGLE_API_KEY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("codetrial should exit");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&cwd);

    assert!(
        !stderr.contains("required configuration file is missing"),
        "the file beside the executable should have answered: {stderr}"
    );
    assert!(
        stderr.contains(config.to_str().unwrap()),
        "and it should be the file the error names: {stderr}"
    );
}

/// Every mode that reads configuration refuses without a file, and says which
/// file it wanted. One test over the modes rather than one test each: the
/// assertion is the same sentence three times, and a mode added to MODES that
/// forgets this belongs in this list rather than in a fourth copy.
///
/// `web` is not here: a missing config is a cold start for it, covered by
/// `binary_web_serves_a_setup_page_when_no_config_exists`.
#[test]
fn binary_modes_that_read_configuration_require_a_primary_config_file() {
    for args in [
        &["run-livekit", "interview-fixed"][..],
        &["check-gemini"][..],
    ] {
        let (code, stdout, stderr) = run_cli_args(args);

        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert!(stdout.is_empty(), "{args:?}: {stdout}");
        assert!(
            stderr.contains("required configuration file is missing"),
            "{args:?}: {stderr}"
        );
    }
}

/// `--config` alone still leaves zero positionals but skips the cold-start
/// path, so the credentials refusal proves `web` was picked, not
/// `run-livekit`/`check-gemini`.
#[test]
fn binary_with_no_arguments_defaults_to_web_mode() {
    let dir = temp_path("default-mode");
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("codetrial.env.local");
    std::fs::write(&config, "CODETRIAL_WEB_ADDR=127.0.0.1:1\n").unwrap();

    let (code, stdout, stderr) = run_cli_args(&["--config", config.to_str().unwrap()]);
    let _ = std::fs::remove_dir_all(dir);

    assert_eq!(code, 1, "{stderr}");
    assert!(stdout.is_empty());
    assert!(
        stderr.contains("missing required LiveKit credentials"),
        "{stderr}"
    );
}

/// The solo self-serve cold start: no config anywhere, so it serves the
/// Setup page instead of refusing.
#[test]
fn binary_web_serves_a_setup_page_when_no_config_exists() {
    let dir = exe_temp_path("cold-start");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = binary_beside(&dir);

    let (addr, _server) = spawn_server(|addr| {
        let mut command = Command::new(&exe);
        command
            .args(["web", "--web-addr", addr])
            .current_dir(&dir)
            .env_remove("LIVEKIT_URL")
            .env_remove("LIVEKIT_API_KEY")
            .env_remove("LIVEKIT_API_SECRET")
            .env_remove("GOOGLE_API_KEY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });
    let response = http_request(
        &addr,
        "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    let _ = std::fs::remove_dir_all(&dir);

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains("Setup"), "{response}");
}

/// Field names are the contract with `submit_setup`'s JSON keys; pinned so
/// page and handler can't drift apart.
#[test]
fn setup_page_renders_a_form_with_all_four_credential_fields() {
    let dir = exe_temp_path("setup-form");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = binary_beside(&dir);

    let (addr, _server) = spawn_server(|addr| {
        let mut command = Command::new(&exe);
        command
            .args(["web", "--web-addr", addr])
            .current_dir(&dir)
            .env_remove("LIVEKIT_URL")
            .env_remove("LIVEKIT_API_KEY")
            .env_remove("LIVEKIT_API_SECRET")
            .env_remove("GOOGLE_API_KEY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });
    let response = http_request(
        &addr,
        "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    let _ = std::fs::remove_dir_all(&dir);

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    for field in [
        "livekitUrl",
        "livekitApiKey",
        "livekitApiSecret",
        "googleApiKey",
    ] {
        assert!(
            response.contains(&format!("name=\"{field}\"")),
            "missing {field} field: {response}"
        );
    }
    assert!(response.contains("/api/setup"), "{response}");
}

/// Accept, validate, write: the mocks answer as working credentials would —
/// LiveKit's `ListRooms` with 200, Gemini's handshake with `setupComplete`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setup_page_accepts_credentials_and_writes_the_primary_config_file() {
    let dir = exe_temp_path("setup-submit");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = binary_beside(&dir);

    let livekit_mock = TcpListener::bind("127.0.0.1:0").unwrap();
    let livekit_addr = livekit_mock.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((mut socket, _)) = livekit_mock.accept() {
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer);
            let _ = socket.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10\r\nConnection: close\r\n\r\n{\"rooms\":[]}",
            );
        }
    });

    let gemini_mock = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gemini_addr = gemini_mock.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((socket, _)) = gemini_mock.accept().await
            && let Ok(mut socket) = accept_async(socket).await
        {
            let _ = socket.next().await;
            let _ = socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    r#"{"setupComplete":{}}"#.into(),
                ))
                .await;
        }
    });

    let (addr, _server) = spawn_server(|addr| {
        let mut command = Command::new(&exe);
        command
            .args(["web", "--web-addr", addr])
            .current_dir(&dir)
            .env("CODETRIAL_GEMINI_LIVE_URL", format!("ws://{gemini_addr}"))
            .env_remove("LIVEKIT_URL")
            .env_remove("LIVEKIT_API_KEY")
            .env_remove("LIVEKIT_API_SECRET")
            .env_remove("GOOGLE_API_KEY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });

    let body = format!(
        r#"{{"livekitUrl":"http://{livekit_addr}","livekitApiKey":"key","livekitApiSecret":"secret","googleApiKey":"google"}}"#
    );
    let response = http_request(
        &addr,
        &format!(
            "POST /api/setup HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );

    let written = std::fs::read_to_string(dir.join("codetrial.env.local"))
        .expect("codetrial.env.local should have been written");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(
        written.contains(&format!("LIVEKIT_URL=http://{livekit_addr}")),
        "{written}"
    );
    assert!(written.contains("LIVEKIT_API_KEY=key"), "{written}");
    assert!(written.contains("LIVEKIT_API_SECRET=secret"), "{written}");
    assert!(written.contains("GOOGLE_API_KEY=google"), "{written}");
}

/// `googleApiKey` is optional, same as an operator's config file: omitting it
/// writes an empty `GOOGLE_API_KEY` and skips the Gemini live check, so no
/// Gemini mock is needed here.
#[test]
fn setup_page_accepts_credentials_without_a_google_api_key() {
    let dir = exe_temp_path("setup-submit-no-google-key");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = binary_beside(&dir);

    let livekit_mock = TcpListener::bind("127.0.0.1:0").unwrap();
    let livekit_addr = livekit_mock.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((mut socket, _)) = livekit_mock.accept() {
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer);
            let _ = socket.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10\r\nConnection: close\r\n\r\n{\"rooms\":[]}",
            );
        }
    });

    let (addr, _server) = spawn_server(|addr| {
        let mut command = Command::new(&exe);
        command
            .args(["web", "--web-addr", addr])
            .current_dir(&dir)
            .env_remove("LIVEKIT_URL")
            .env_remove("LIVEKIT_API_KEY")
            .env_remove("LIVEKIT_API_SECRET")
            .env_remove("GOOGLE_API_KEY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });

    let body = format!(
        r#"{{"livekitUrl":"http://{livekit_addr}","livekitApiKey":"key","livekitApiSecret":"secret","googleApiKey":""}}"#
    );
    let response = http_request(
        &addr,
        &format!(
            "POST /api/setup HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );

    let written = std::fs::read_to_string(dir.join("codetrial.env.local"))
        .expect("codetrial.env.local should have been written");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(written.contains("GOOGLE_API_KEY=\n"), "{written}");
}

/// Drop-and-rebind means a brief gap, so this polls rather than asserting
/// the next request lands immediately. `/api/session` only exists on the
/// full app, so 200 there proves the switch happened.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setup_page_continues_serving_the_full_app_after_a_successful_submission() {
    let dir = exe_temp_path("setup-continue");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = binary_beside(&dir);

    let livekit_mock = TcpListener::bind("127.0.0.1:0").unwrap();
    let livekit_addr = livekit_mock.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((mut socket, _)) = livekit_mock.accept() {
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer);
            let _ = socket.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10\r\nConnection: close\r\n\r\n{\"rooms\":[]}",
            );
        }
    });

    let gemini_mock = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gemini_addr = gemini_mock.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((socket, _)) = gemini_mock.accept().await
            && let Ok(mut socket) = accept_async(socket).await
        {
            let _ = socket.next().await;
            let _ = socket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    r#"{"setupComplete":{}}"#.into(),
                ))
                .await;
        }
    });

    let (addr, _server) = spawn_server(|addr| {
        let mut command = Command::new(&exe);
        command
            .args(["web", "--web-addr", addr])
            .current_dir(&dir)
            .env("CODETRIAL_GEMINI_LIVE_URL", format!("ws://{gemini_addr}"))
            .env_remove("LIVEKIT_URL")
            .env_remove("LIVEKIT_API_KEY")
            .env_remove("LIVEKIT_API_SECRET")
            .env_remove("GOOGLE_API_KEY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });

    let body = format!(
        r#"{{"livekitUrl":"http://{livekit_addr}","livekitApiKey":"key","livekitApiSecret":"secret","googleApiKey":"google"}}"#
    );
    let submit_response = http_request(
        &addr,
        &format!(
            "POST /api/setup HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );
    assert!(
        submit_response.starts_with("HTTP/1.1 200 OK"),
        "{submit_response}"
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut response = String::new();
    while Instant::now() < deadline {
        if let Some(candidate) = try_http(
            &addr,
            "GET /api/session HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        ) {
            response = candidate;
            if response.starts_with("HTTP/1.1 200 OK") {
                break;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }

    let _ = std::fs::remove_dir_all(&dir);

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains(r#""loginRequired":true"#), "{response}");
}

/// A missing field is refused before anything touches disk.
#[test]
fn setup_page_rejects_a_submission_missing_a_field() {
    let dir = exe_temp_path("setup-missing-field");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = binary_beside(&dir);

    let (addr, _server) = spawn_server(|addr| {
        let mut command = Command::new(&exe);
        command
            .args(["web", "--web-addr", addr])
            .current_dir(&dir)
            .env_remove("LIVEKIT_URL")
            .env_remove("LIVEKIT_API_KEY")
            .env_remove("LIVEKIT_API_SECRET")
            .env_remove("GOOGLE_API_KEY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });

    // `livekitUrl` is absent entirely; the others are merely blank, so this
    // exercises both ways a field can fail to be there.
    let body = r#"{"livekitApiKey":"","livekitApiSecret":"secret","googleApiKey":"google"}"#;
    let response = http_request(
        &addr,
        &format!(
            "POST /api/setup HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );

    let written = dir.join("codetrial.env.local").exists();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );
    assert!(response.contains("livekitUrl"), "{response}");
    assert!(
        !written,
        "a rejected submission must not write a config file"
    );
}

/// `CODETRIAL_GEMINI_LIVE_URL` redirects validation to a local mock that
/// never sends `setupComplete` — a rejected key. LiveKit's mock has to pass
/// so this isolates the Gemini rejection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setup_page_rejects_a_google_api_key_that_fails_live_validation() {
    let dir = exe_temp_path("setup-bad-gemini-key");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = binary_beside(&dir);

    let livekit_mock = TcpListener::bind("127.0.0.1:0").unwrap();
    let livekit_addr = livekit_mock.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((mut socket, _)) = livekit_mock.accept() {
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer);
            let _ = socket.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10\r\nConnection: close\r\n\r\n{\"rooms\":[]}",
            );
        }
    });

    let gemini_mock = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let gemini_addr = gemini_mock.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((socket, _)) = gemini_mock.accept().await
            && let Ok(mut socket) = accept_async(socket).await
        {
            let _ = socket.close(None).await;
        }
    });

    let (addr, _server) = spawn_server(|addr| {
        let mut command = Command::new(&exe);
        command
            .args(["web", "--web-addr", addr])
            .current_dir(&dir)
            .env("CODETRIAL_GEMINI_LIVE_URL", format!("ws://{gemini_addr}"))
            .env_remove("LIVEKIT_URL")
            .env_remove("LIVEKIT_API_KEY")
            .env_remove("LIVEKIT_API_SECRET")
            .env_remove("GOOGLE_API_KEY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });

    let body = format!(
        r#"{{"livekitUrl":"http://{livekit_addr}","livekitApiKey":"key","livekitApiSecret":"secret","googleApiKey":"bad-key"}}"#
    );
    let response = http_request(
        &addr,
        &format!(
            "POST /api/setup HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );

    let written = dir.join("codetrial.env.local").exists();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );
    assert!(response.contains("googleApiKey"), "{response}");
    assert!(!written, "a rejected key must not write a config file");
}

/// LiveKit is validated too, via `ListRooms`; the mock refuses every
/// request, standing in for a wrong key/secret.
#[test]
fn setup_page_rejects_livekit_credentials_that_fail_live_validation() {
    let dir = exe_temp_path("setup-bad-livekit");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = binary_beside(&dir);

    let mock = TcpListener::bind("127.0.0.1:0").unwrap();
    let mock_addr = mock.local_addr().unwrap();
    thread::spawn(move || {
        if let Ok((mut socket, _)) = mock.accept() {
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer);
            let _ = socket.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
    });

    let (addr, _server) = spawn_server(|addr| {
        let mut command = Command::new(&exe);
        command
            .args(["web", "--web-addr", addr])
            .current_dir(&dir)
            .env_remove("LIVEKIT_URL")
            .env_remove("LIVEKIT_API_KEY")
            .env_remove("LIVEKIT_API_SECRET")
            .env_remove("GOOGLE_API_KEY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });

    let body = format!(
        r#"{{"livekitUrl":"http://{mock_addr}","livekitApiKey":"key","livekitApiSecret":"secret","googleApiKey":"google"}}"#
    );
    let response = http_request(
        &addr,
        &format!(
            "POST /api/setup HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );

    let written = dir.join("codetrial.env.local").exists();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "{response}"
    );
    assert!(response.contains("livekitUrl"), "{response}");
    assert!(
        !written,
        "a rejected LiveKit credential must not write a config file"
    );
}

#[test]
fn binary_web_refuses_a_public_listener_without_a_session_secret() {
    let config_dir = temp_path("serve-public-default-secret");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config = config_dir.join("public.env");
    std::fs::write(
        &config,
        "LIVEKIT_URL=wss://example\nLIVEKIT_API_KEY=key\nLIVEKIT_API_SECRET=secret\nGOOGLE_API_KEY=google\n",
    )
    .unwrap();

    let (code, stdout, stderr) = with_free_addr(|addr| {
        let port = addr.rsplit_once(':').unwrap().1;
        let result = run_cli_until_exit(&[
            "web",
            "--web-addr",
            &format!("0.0.0.0:{port}"),
            "--config",
            config.to_str().unwrap(),
        ]);
        (!result.2.contains("failed to bind")).then_some(result)
    });
    let _ = std::fs::remove_dir_all(&config_dir);

    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(
        stderr.contains("SESSION_SECRET must be set to bind 0.0.0.0"),
        "{stderr}"
    );
}

/// The built-in `SESSION_SECRET` is not a weak key, it is a published one, so a
/// production start on it has to refuse rather than mint forgeable cookies.
///
/// `serve` carried the only integration test that ever set
/// `NODE_ENV=production`, and deleting that mode took the coverage with it:
/// this guard sits in `run_web` and is reachable only by starting the binary.
/// The address guard below is a different path and does not stand in for it.
#[test]
fn binary_web_refuses_production_without_a_session_secret() {
    let dir = temp_path("web-production-secret");
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("production.env");
    std::fs::write(
        &config,
        "LIVEKIT_URL=wss://example\nLIVEKIT_API_KEY=key\nLIVEKIT_API_SECRET=secret\nGOOGLE_API_KEY=google\nNODE_ENV=production\n",
    )
    .unwrap();

    // No free-port dance: the refusal comes before the bind, so no listener is
    // ever created and there is no port to race for.
    let (code, stdout, stderr) = run_cli_until_exit(&[
        "web",
        "--web-addr",
        "127.0.0.1:0",
        "--config",
        config.to_str().unwrap(),
    ]);
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(
        stderr.contains("SESSION_SECRET must be set when NODE_ENV=production"),
        "{stderr}"
    );
}

/// And the other direction, so the guard cannot be inverted without a test
/// noticing: production with a secret of the operator's own is the supported
/// deployment and has to serve.
#[test]
fn binary_web_serves_in_production_with_a_session_secret() {
    let dir = temp_path("web-production-serves");
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("production.env");
    std::fs::write(
        &config,
        format!(
            "LIVEKIT_URL=wss://example\nLIVEKIT_API_KEY=key\nLIVEKIT_API_SECRET=secret\nGOOGLE_API_KEY=google\nNODE_ENV=production\nSESSION_SECRET=a-real-secret\nCODETRIAL_DB_PATH={}/accounts.db\n",
            dir.display()
        ),
    )
    .unwrap();

    let (addr, _server) = spawn_server(|addr| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_codetrial"));
        command
            .args(["web", "--web-addr", addr, "--config"])
            .arg(config.to_str().unwrap());
        command
    });
    let response = http_request(
        &addr,
        "GET /api/session HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    let _ = std::fs::remove_dir_all(&dir);

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
}

/// `serve` used to read the proxy-hop count straight from the process
/// environment, so a value set in the config file was silently a no-op and the
/// rate limiter keyed on the proxy instead of the client. Asserted against the
/// source because reaching it behaviorally needs a running proxy;
/// `binary_web_refuses_a_public_listener_without_a_session_secret` covers a
/// startup refusal read from the same values, for real.
#[test]
fn binary_web_reads_deployment_keys_from_the_config_file() {
    let source = std::fs::read_to_string("src/main.rs").unwrap();

    // Ends at the closing brace in column zero rather than at whatever item
    // happens to follow. Keying on the next `async fn` meant deleting the
    // function that used to sit there silently emptied this test's haystack.
    let serve = source
        .split_once("fn run_web(")
        .expect("run_web should exist")
        .1
        .split_once("\n}\n")
        .expect("run_web should end")
        .0;

    assert!(serve.contains("trusted_proxy_hops(&values)"), "{serve}");
    assert!(!serve.contains("std::env::var"), "{serve}");
}

/// `spawn_server` decides whether a failed start was a lost port race by
/// looking for "Address already in use" in the child's stderr. That only works
/// while the child's stderr is piped, and one spawning test used to build its
/// Command without piping, so its retry could never fire. Pinned here rather
/// than left to whoever writes the next spawning test.
#[test]
fn a_spawned_server_pipes_stderr_so_a_bind_failure_can_be_read() {
    // No arguments: it exits on a usage error immediately, which is all this
    // needs. The assertion is about the pipe, not about the server.
    let child = ServerProcess::spawn(&mut Command::new(env!("CARGO_BIN_EXE_codetrial")));
    assert!(
        child.stderr.is_some(),
        "a child whose stderr is inherited reports an empty reason, and a lost \
         port race is then indistinguishable from the behavior under test failing"
    );
}

#[test]
fn binary_web_does_not_require_github_oauth_config() {
    let dir = temp_path("partial-login");
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("partial.env");

    // Absolute, because the path is relative to the spawned binary's working
    // directory: a bare `accounts.db` puts every concurrent test on one file
    // and its WAL sidecars.
    std::fs::write(
        &config,
        format!(
            "LIVEKIT_URL=wss://example\nLIVEKIT_API_KEY=key\nLIVEKIT_API_SECRET=secret\nGOOGLE_API_KEY=google\nCODETRIAL_DB_PATH={}/accounts.db\n",
            dir.display()
        ),
    )
    .unwrap();

    let (addr, _server) = spawn_server(|addr| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_codetrial"));
        command
            .args(["web", "--web-addr", addr, "--config"])
            .arg(config.to_str().unwrap());
        command
    });
    let response = http_request(
        &addr,
        "GET /api/session HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    let _ = std::fs::remove_dir_all(&dir);

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.contains(r#""loginRequired":true"#), "{response}");
}

#[test]
fn binary_web_reports_bind_failure_after_config_validation() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("occupied port should bind");
    let addr = listener.local_addr().unwrap().to_string();
    let dir = temp_path("occupied-port");
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("codetrial.env.local");
    write_config(&config, &dir, "127.0.0.1:1");
    let (code, stdout, stderr) = run_cli_args(&[
        "web",
        "--web-addr",
        &addr,
        "--config",
        config.to_str().unwrap(),
    ]);
    let _ = std::fs::remove_dir_all(dir);

    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("failed to bind"), "{stderr}");
}

#[test]
fn binary_agent_modes_reject_usage_errors_with_exit_two() {
    for args in [
        &["unknown"][..],
        &["web", "extra"],
        &["web", "--bad", "value"],
        &["web", "--web-addr"],
        &["web", "--duration-min", "nope"],
        &["web", "--duration-min", "0"],
        &["run-livekit", "room", "extra"],
        &["check-gemini", "extra"],
    ] {
        let (code, stdout, stderr) = run_cli_args(args);

        assert_eq!(code, 2, "{args:?}: {stderr}");
        assert!(stdout.is_empty());
        assert!(!stderr.is_empty());
    }
}

#[test]
fn binary_agent_web_uses_config_and_cli_override_precedence() {
    let file_dir = temp_path("file");
    let cli_dir = temp_path("cli");
    let config_path = temp_path("config");
    std::fs::create_dir_all(&file_dir).expect("file web dir should create");
    std::fs::create_dir_all(&cli_dir).expect("cli web dir should create");
    std::fs::write(file_dir.join("index.html"), "from file").expect("file index should write");
    std::fs::write(cli_dir.join("index.html"), "from cli").expect("cli index should write");

    // The config file names an address the CLI flag must override.
    write_config(&config_path, &file_dir, &free_addr());

    let (cli_addr, mut child) = spawn_server(|addr| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_codetrial"));
        command
            .args([
                "web",
                "--config",
                config_path.to_str().unwrap(),
                "--web-addr",
                addr,
                "--web-dir",
                cli_dir.to_str().unwrap(),
                "--room-prefix",
                "cli",
                "--duration-min",
                "33",
            ])
            .env("CODETRIAL_WEB_DIR", "/missing-env-dir")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });

    let root = http_request(
        &cli_addr,
        "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(root.contains("from cli"), "{root}");

    let body = r#"{"problemId":"two-sum","durationMin":20}"#;
    let token = http_request(
        &cli_addr,
        &format!(
            "POST /api/token HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );
    assert!(token.contains("401 Unauthorized"), "{token}");
    assert!(token.contains("GitHub username"), "{token}");

    stop_child(&mut child);
    let _ = std::fs::remove_dir_all(file_dir);
    let _ = std::fs::remove_dir_all(cli_dir);
    let _ = std::fs::remove_file(config_path);
}

#[test]
fn binary_agent_web_loads_primary_config_from_the_current_directory() {
    let cwd = temp_path("cwd");
    let web_dir = cwd.join("site");
    std::fs::create_dir_all(&web_dir).expect("web dir should create");
    std::fs::write(web_dir.join("index.html"), "from local config").expect("index should write");
    write_config(&cwd.join("codetrial.env.local"), &web_dir, "127.0.0.1:1");

    let (cli_addr, mut child) = spawn_server(|addr| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_codetrial"));
        command
            .args(["web", "--web-addr", addr, "--room-prefix", "cli"])
            .current_dir(&cwd)
            .env_remove("LIVEKIT_URL")
            .env_remove("LIVEKIT_API_KEY")
            .env_remove("LIVEKIT_API_SECRET")
            .env_remove("GOOGLE_API_KEY")
            .env_remove("CODETRIAL_WEB_DIR")
            .env_remove("CODETRIAL_WEB_ADDR")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    });

    let root = http_request(
        &cli_addr,
        "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(root.contains("from local config"), "{root}");

    let body = r#"{"problemId":"two-sum","durationMin":20}"#;
    let token = http_request(
        &cli_addr,
        &format!(
            "POST /api/token HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );
    assert!(token.contains("401 Unauthorized"), "{token}");
    assert!(token.contains("GitHub username"), "{token}");

    stop_child(&mut child);
    let _ = std::fs::remove_dir_all(cwd);
}

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn run_cli_args(args: &[&str]) -> (i32, String, String) {
    run_cli_args_with_env(args, &[])
}

fn run_cli_args_with_env(args: &[&str], envs: &[(&str, &str)]) -> (i32, String, String) {
    let cwd = temp_path("empty-cwd");
    std::fs::create_dir_all(&cwd).expect("empty cwd should create");
    let output = Command::new(env!("CARGO_BIN_EXE_codetrial"))
        .args(args)
        .current_dir(&cwd)
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
        .stderr(Stdio::piped())
        .output()
        .expect("codetrial should exit");
    let _ = std::fs::remove_dir_all(cwd);

    (
        output.status.code().unwrap_or_default(),
        String::from_utf8(output.stdout).expect("stdout should be UTF-8"),
        String::from_utf8(output.stderr).expect("stderr should be UTF-8"),
    )
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
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
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

/// Every mode that reads configuration refuses without a file, and says which
/// file it wanted. One test over the modes rather than one test each: the
/// assertion is the same sentence three times, and a mode added to MODES that
/// forgets this belongs in this list rather than in a fourth copy.
///
/// `serve` is not here. It needs a free address to reach the same refusal, so
/// it carries its own test below.
#[test]
fn binary_modes_that_read_configuration_require_a_primary_config_file() {
    for args in [
        &["run-livekit", "interview-fixed"][..],
        &["check-gemini"][..],
        &["web"][..],
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

#[test]
fn binary_web_requires_a_primary_config_file() {
    let (code, stdout, stderr) = with_free_addr(|addr| {
        let result = run_cli_args(&["web", "--web-addr", addr]);
        (!result.2.contains("failed to bind")).then_some(result)
    });

    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains("required configuration file is missing"));
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
        let result = run_cli_args(&[
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

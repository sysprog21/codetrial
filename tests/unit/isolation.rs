use std::time::{Duration, Instant};

pub(crate) fn isolated_checked(
    name: &str,
    marker: &str,
    loopback_http: bool,
    check: impl FnOnce(&std::process::Output),
) -> bool {
    if std::env::var(marker).as_deref() == Ok(name) {
        return false;
    }
    let guard = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    guard.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", guard.local_addr().unwrap());
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", name, "--nocapture"])
        .env(marker, name)
        .env("NO_PROXY", if loopback_http { "127.0.0.1" } else { "" })
        .env("no_proxy", if loopback_http { "127.0.0.1" } else { "" })
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env(key, &proxy);
    }
    let mut child = command.spawn().unwrap();

    // Drained while the child runs: a child that writes more than a pipe holds
    // (64 KiB on Linux) blocks in its write, and polling only for its exit then
    // reads that block as a hang.
    let drain = |stream: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
    );
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(20) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "isolated test child timed out: {}",
                String::from_utf8_lossy(&stderr.join().unwrap())
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = std::process::Output {
        status,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    };
    assert_eq!(
        guard.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "an isolated test must not contact an external provider"
    );
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line == "running 1 test")
            && stdout.lines().any(|line| {
                line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;")
            }),
        "isolated child must execute exactly one passing test: {name}\n{stdout}"
    );
    check(&output);
    true
}

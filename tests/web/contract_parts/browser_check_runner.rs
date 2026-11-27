// The browser-check runner's tests, a module of contract.rs rather than an
// included part so that `cargo fmt` formats it with the rest of the crate.

use super::*;

#[tokio::test]
#[ignore = "browser-dependent: hermetic and about 30s idle, but a real browser \
            under a loaded machine has been measured failing on page.goto past \
            a two minute navigation timeout. CI runs it as its own step; \
            locally, `cargo test --test web -- --ignored` when the machine is \
            not busy"]
async fn browser_check_accepts_running_rust_server_offline_interview() {
    let (mut config, cookie, db_path) = signed_in_web_config("browser-offline");
    config.web_dir = Path::new("web").to_path_buf();
    config.pool = Default::default();
    let (base, server) = spawn_web_server(config).await;
    let output = tokio::task::spawn_blocking(move || {
        Command::new("sh")
            .arg("scripts/browser-check.sh")
            .env("CODETRIAL_WEB_URL", base)
            .env("BROWSER_CHECK_AGENT", "offline")
            .env("BROWSER_CHECK_SESSION_COOKIE", cookie)
            // The mock, not godbolt.org. The live service is what the default
            // reaches for, and it turned this into a four minute test that a
            // network hiccup could fail: the mock runs the same editor and
            // results pipeline against fixed responses in a fraction of it.
            // Whether godbolt's API still looks the way this code expects is a
            // real question, but it is not one a commit should be blocked on.
            .env("BROWSER_CHECK_COMPILER_EXPLORER_BASE_URL", "mock")
            .output()
    })
    .await
    .unwrap()
    .expect("browser check should run");

    // Exit 3 is browser-check.sh saying Playwright is not installed. Skipped
    // rather than failed, matching tests/browser/face-detector.test.js: the
    // suite has to stay runnable without an 800 MB download. CI installs
    // Chromium, so this arm fails there rather than hide the lost check.
    if output.status.code() == Some(3) {
        assert_ne!(
            std::env::var("CI").as_deref(),
            Ok("true"),
            "Playwright is not installed, and CI must run this test: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        eprintln!(
            "skipping offline browser check: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        server.shutdown().await;
        remove_database(db_path).await;
        return;
    }

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    server.shutdown().await;
    remove_database(db_path).await;
}

#[tokio::test]
#[ignore = "credentialed browser check; requires LiveKit and Google env"]
async fn browser_check_accepts_running_rust_server_with_rust_agent() {
    let (mut config, cookie, db_path) = signed_in_web_config("browser-rust");
    config.web_dir = Path::new("web").to_path_buf();
    config.pool = primary_pool(
        &std::env::var("LIVEKIT_URL").expect("set LIVEKIT_URL"),
        &std::env::var("LIVEKIT_API_KEY").expect("set LIVEKIT_API_KEY"),
        &std::env::var("LIVEKIT_API_SECRET").expect("set LIVEKIT_API_SECRET"),
    );
    let (base, server) = spawn_web_server(config).await;
    let output = tokio::task::spawn_blocking(move || {
        Command::new("sh")
            .arg("scripts/browser-check.sh")
            .env("CODETRIAL_WEB_URL", base)
            .env("BROWSER_CHECK_AGENT", "rust")
            .env("BROWSER_CHECK_SESSION_COOKIE", cookie)
            .output()
    })
    .await
    .unwrap()
    .expect("browser check should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    server.shutdown().await;
    remove_database(db_path).await;
}

#[test]
fn browser_check_loads_credentials_for_external_rust_server() {
    let env_file = std::env::temp_dir().join(format!(
        "codetrial-env-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &env_file,
        "LIVEKIT_URL=wss://example.livekit.cloud\nLIVEKIT_API_KEY=key\nLIVEKIT_API_SECRET=secret\nGOOGLE_API_KEY=google\n",
    )
    .unwrap();

    let output = Command::new("sh")
        .arg("scripts/browser-check.sh")
        .env("CODETRIAL_WEB_URL", "http://127.0.0.1:1")
        .env("CODETRIAL_CONFIG_ENV", &env_file)
        .env("BROWSER_CHECK_AGENT", "rust")
        .env("BROWSER_CHECK_VALIDATE_ENV_ONLY", "1")
        .env_remove("LIVEKIT_URL")
        .env_remove("LIVEKIT_API_KEY")
        .env_remove("LIVEKIT_API_SECRET")
        .env_remove("GOOGLE_API_KEY")
        .output()
        .expect("browser check should run");

    fs::remove_file(env_file).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Runs browser-check.sh against stubbed cargo, curl and node, and returns
/// its output, the order the stubs were called in, and the build line the
/// script should have issued.
#[cfg(unix)]
fn run_browser_check_with_stub_cargo(build_fails: bool) -> (std::process::Output, String, String) {
    use std::os::unix::fs::PermissionsExt;

    let root = unique_temp_path("browser-check-build", "");
    let scripts = root.join("scripts");
    let bin = root.join("bin");
    fs::create_dir_all(&scripts).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::copy("scripts/browser-check.sh", scripts.join("browser-check.sh")).unwrap();
    for helper in ["session-cookie.sh", "wait-for-server.sh"] {
        fs::copy(format!("scripts/{helper}"), scripts.join(helper)).unwrap();
    }

    for (path, source) in [
        (scripts.join("fetch-vendor.sh"), "#!/bin/sh\nexit 0\n"),
        (
            bin.join("cargo"),
            r#"#!/bin/sh
echo "$*" >> "$CHECK_ROOT/events"
if [ "$1" = build ] && [ "${BUILD_FAILS:-}" ]; then
    echo compiler-failed >&2
    exit 42
fi
"#,
        ),
        // Ready once the server has been launched, which under `set -e` means
        // the build before it succeeded.
        (
            bin.join("curl"),
            "#!/bin/sh\necho curl >> \"$CHECK_ROOT/events\"\ngrep -q '^run ' \"$CHECK_ROOT/events\"\n",
        ),
        (
            bin.join("node"),
            "#!/bin/sh\necho driver >> \"$CHECK_ROOT/events\"\n",
        ),
    ] {
        fs::write(&path, source).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut command = Command::new("sh");
    command
        .arg(scripts.join("browser-check.sh"))
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("CHECK_ROOT", &root)
        .env("BROWSER_CHECK_AGENT", "home")
        .env("BROWSER_CHECK_FLOW", "avatar")
        .env("BROWSER_CHECK_SESSION_COOKIE", "codetrial_session=test")
        .env("PLAYWRIGHT_PATH", "stub")
        .env("PORT", "3100")
        .env_remove("CODETRIAL_WEB_URL");
    if build_fails {
        command.env("BUILD_FAILS", "1");
    }
    let output = command.output().unwrap();
    let events = fs::read_to_string(root.join("events")).unwrap_or_default();

    // The same target `cargo run` launches, so the launch has nothing left to
    // compile inside the readiness deadline.
    let build = format!(
        "build --manifest-path {}/Cargo.toml --bin codetrial\n",
        root.display()
    );
    fs::remove_dir_all(root).unwrap();
    (output, events, build)
}

#[test]
#[cfg(unix)]
fn browser_check_builds_before_waiting_for_server() {
    let (output, events, build) = run_browser_check_with_stub_cargo(false);

    assert!(output.status.success(), "{output:?}");
    assert!(events.starts_with(&build), "{events}");
    assert!(events.contains("\nrun "), "{events}");
    assert!(events.contains("driver\n"), "{events}");
}

#[test]
#[cfg(unix)]
fn browser_check_stops_on_a_failed_build() {
    let (output, events, build) = run_browser_check_with_stub_cargo(true);

    assert_eq!(output.status.code(), Some(42), "{output:?}");
    assert_eq!(events, build);
    assert!(String::from_utf8_lossy(&output.stderr).contains("compiler-failed"));
}

#[test]
#[cfg(unix)]
fn browser_check_rejects_unknown_agent_before_building() {
    use std::os::unix::fs::PermissionsExt;

    // A cargo that records being called, so the test fails if the build ever
    // moves ahead of the refusal. The env-only flag keeps a refusal that went
    // missing from going on to fetch, build and launch for real.
    let bin = unique_temp_path("browser-check-agent", "");
    fs::create_dir_all(&bin).unwrap();
    let cargo = bin.join("cargo");
    fs::write(&cargo, "#!/bin/sh\ntouch \"$(dirname \"$0\")/called\"\n").unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new("sh")
        .arg("scripts/browser-check.sh")
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("BROWSER_CHECK_AGENT", "bogus")
        .env("BROWSER_CHECK_FLOW", "avatar")
        .env("BROWSER_CHECK_VALIDATE_ENV_ONLY", "1")
        .output()
        .expect("browser check should run");
    let built = bin.join("called").exists();
    fs::remove_dir_all(bin).unwrap();

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("BROWSER_CHECK_AGENT must be"));
    assert!(
        !built,
        "an unknown agent should be refused before cargo runs"
    );
}

#[test]
fn browser_check_prints_server_log_on_node_failure() {
    let script = fs::read_to_string("scripts/browser-check.sh").unwrap();
    let driver = fs::read_to_string("scripts/browser-check.cjs").unwrap();

    assert!(script.contains("SERVER_LOG=\"$SERVER_LOG\""));
    assert!(script.contains("scripts/browser-check.cjs"));
    assert!(driver.contains("fs.readFileSync(process.env.SERVER_LOG"));
}

#[test]
fn browser_check_bounds_room_service_requests() {
    let driver = fs::read_to_string("scripts/livekit-room-service.cjs").unwrap();
    let room_service = source_block(
        &driver,
        "async function roomService",
        "\n/// Protobuf JSON omits",
    );

    assert!(room_service.contains("signal: AbortSignal.timeout(30000)"));
}

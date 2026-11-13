//! What a Setup submission writes into the config file, first-time and
//! through Change keys.
//!
//! Split out of `tests/cli.rs`, which is still the test target: cargo
//! discovers only `tests/*.rs`, so this compiles as a module of that one
//! binary rather than relinking the crate for a file of its own.

use super::*;

pub(super) fn reserve_gemini_listener() -> tokio::net::TcpListener {
    let listener = reserve_listener();
    listener.set_nonblocking(true).unwrap();
    tokio::net::TcpListener::from_std(listener).unwrap()
}

/// Accept, validate, write: the mocks answer as working credentials would,
/// LiveKit's `ListRooms` with 200, Gemini's handshake with `setupComplete`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setup_page_accepts_credentials_and_writes_the_primary_config_file() {
    let dir = exe_temp_path("setup-submit");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = binary_beside(&dir);

    let livekit_addr = spawn_livekit_room_service("key", "secret");

    let gemini_mock = reserve_gemini_listener();
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

    let (addr, _server) = spawn_cold_start(
        &exe,
        &dir,
        &[("CODETRIAL_GEMINI_LIVE_URL", format!("ws://{gemini_addr}"))],
    );

    let body = format!(
        r#"{{"livekitUrl":"http://{livekit_addr}","livekitApiKey":"key","livekitApiSecret":"secret","googleApiKey":"google"}}"#
    );
    let response = http_request(
        &addr,
        &format!(
            "POST /api/setup HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );

    let written = std::fs::read_to_string(dir.join("config").join("codetrial.env.local"))
        .expect("codetrial.env.local should have been written");

    // The file holds two secrets, so the umask does not get to decide who reads
    // it. Windows has no mode to check and inherits the folder's ACL.
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(dir.join("config").join("codetrial.env.local"))
            .expect("the written config should be readable")
            .permissions()
            .mode()
            & 0o777
    };
    let _ = std::fs::remove_dir_all(&dir);

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    #[cfg(unix)]
    assert_eq!(mode, 0o600, "{mode:o}");
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

    let livekit_addr = spawn_livekit_room_service("key", "secret");

    let (addr, _server) = spawn_cold_start(&exe, &dir, &[]);

    let body = format!(
        r#"{{"livekitUrl":"http://{livekit_addr}","livekitApiKey":"key","livekitApiSecret":"secret","googleApiKey":""}}"#
    );
    let response = http_request(
        &addr,
        &format!(
            "POST /api/setup HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        ),
    );

    let written = std::fs::read_to_string(dir.join("config").join("codetrial.env.local"))
        .expect("codetrial.env.local should have been written");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");

    // Absent, not empty. `read_config_file` keeps an empty value and
    // `load_values` lays the file over the environment, so the line would erase
    // a `GOOGLE_API_KEY` the operator exported.
    assert!(!written.contains("GOOGLE_API_KEY"), "{written}");
}

/// Change keys from end to end: a standalone launch prints its code, and a save
/// with that code rewrites the config the launch read, comment and all. The new
/// Google key is proved with the model that config sets, and it replaces the
/// config's key list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn change_keys_rewrites_the_config_the_launch_read() {
    use std::io::{BufRead, BufReader};

    let dir = exe_temp_path("change-keys");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = binary_beside(&dir);
    let livekit = spawn_livekit_room_service("new-key", "secret");

    // Answers the probe's setup message and hands back the model it named.
    let gemini_mock = reserve_gemini_listener();
    let gemini_addr = gemini_mock.local_addr().unwrap();
    let gemini = tokio::spawn(async move {
        let (socket, _) = gemini_mock.accept().await.unwrap();
        let mut socket = accept_async(socket).await.unwrap();
        let setup = socket.next().await.unwrap().unwrap();
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"setupComplete":{}}"#.into(),
            ))
            .await
            .unwrap();
        let setup: serde_json::Value = serde_json::from_str(setup.to_text().unwrap()).unwrap();
        setup["setup"]["model"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    });

    let config = dir.join("codetrial.env.local");
    let contents = |livekit_key: &str, google: &str| {
        format!(
            "# keep this\nLIVEKIT_URL=http://{livekit}\nLIVEKIT_API_KEY={livekit_key}\n\
             LIVEKIT_API_SECRET=secret\nGEMINI_LIVE_MODEL=fixture-live\n{google}"
        )
    };
    std::fs::write(
        &config,
        contents("old-key", "GOOGLE_API_KEYS=old-a,old-b\n"),
    )
    .unwrap();
    let (addr, mut server) = spawn_server(|addr| {
        let mut command = Command::new(&exe);
        scrubbed(&mut command)
            .args([
                "web",
                "--data-dir",
                dir.to_str().unwrap(),
                "--web-addr",
                addr,
            ])
            .current_dir(&dir)
            .env("CODETRIAL_GEMINI_LIVE_URL", format!("ws://{gemini_addr}"))
            .stdout(Stdio::piped());
        command
    });

    // Read on a thread of its own, so a launch that never prints the code fails
    // the wait below instead of blocking the test on a pipe that stays open.
    let printed = format!("codetrial: to change keys at http://{addr}/setup, enter ");
    let stdout = server.stdout.take().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(code) = line.strip_prefix(&printed) {
                let _ = send.send(code.to_string());
            }
        }
    });
    let code = receive.recv_timeout(Duration::from_secs(10)).unwrap();
    let body =
        format!(r#"{{"code":"{code}","livekitApiKey":"new-key","googleApiKey":"new-google"}}"#);
    let response = http_request(
        &addr,
        &format!(
            "POST /api/setup HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    let written = std::fs::read_to_string(&config).unwrap();
    let model = tokio::time::timeout(Duration::from_secs(10), gemini)
        .await
        .unwrap()
        .unwrap();
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert_eq!(model, "models/fixture-live");
    assert_eq!(
        written,
        contents("new-key", "GOOGLE_API_KEYS=\nGOOGLE_API_KEY=new-google\n")
    );
}

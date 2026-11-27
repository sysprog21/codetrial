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

/// Change keys for a launcher's child, as the desktop app starts it: the code
/// comes in `CODETRIAL_CHANGE_KEYS_CODE`, a save with it rewrites the config,
/// and the code never reaches stdout, which the app writes to a log file. What
/// does reach it after the save is the line the app restarts the child on.
#[test]
fn a_child_saves_with_the_code_its_launcher_handed_over_and_never_prints_it() {
    save_as_a_child("GOOGLE_API_KEY=old-google\n", true, true);
}

/// A program that starts the server as its child and hands over a code, but
/// has not said it restarts the child, keeps a server that serves on its old
/// keys after the save, as before: it never closes admission for a restart
/// that no one performs.
#[test]
fn a_child_whose_launcher_does_not_restart_it_never_asks_for_one() {
    save_as_a_child("GOOGLE_API_KEY=old-google\n", false, false);
}

/// A child without a Google key serves interviews hosted elsewhere, which it
/// cannot see, so it never says that none is running: the person restarts it.
#[test]
fn a_child_without_an_interviewer_of_its_own_never_asks_for_a_restart() {
    save_as_a_child("", true, false);
}

fn save_as_a_child(google: &str, opts_in: bool, restarts: bool) {
    use std::io::{BufRead, BufReader};

    const CODE: &str = "0123456789abcdef0123456789abcdef";
    const RESTART: &str = "codetrial: keys saved and no interview running: restart now";
    let dir = exe_temp_path("change-keys-child");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = binary_beside(&dir);
    let livekit = spawn_livekit_room_service("new-key", "secret");

    let config = dir.join("codetrial.env.local");
    let contents = |livekit_key: &str| {
        format!(
            "LIVEKIT_URL=http://{livekit}\nLIVEKIT_API_KEY={livekit_key}\n\
             LIVEKIT_API_SECRET=secret\n{google}"
        )
    };
    std::fs::write(&config, contents("old-key")).unwrap();
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
            .env("CODETRIAL_EXIT_WITH_STDIN", "1")
            .env("CODETRIAL_CHANGE_KEYS_CODE", CODE)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        if opts_in {
            command.env("CODETRIAL_RESTART_AFTER_SAVE", "1");
        }
        command
    });
    let stdout = server.stdout.take().unwrap();
    let (sender, lines) = std::sync::mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let _ = sender.send(line);
        }
    });

    let save = |code: &str| {
        let body = format!(r#"{{"code":"{code}","livekitApiKey":"new-key"}}"#);
        format!(
            "POST /api/setup HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    };
    let refused = http_request(&addr, &save("ffffffffffffffffffffffffffffffff"));

    // Sent from a thread of its own while this one plays the launcher, which
    // closes the server's stdin, and so ends it, the moment the restart line
    // arrives: the response has to be out by then.
    let saving = {
        let (addr, request) = (addr.clone(), save(CODE));
        thread::spawn(move || http_request(&addr, &request))
    };

    // Printed once nothing is running, which here is at once. Where it never
    // comes, a quiet wait as long as it would take is what shows that.
    let quiet = Duration::from_secs(if restarts { 10 } else { 2 });
    let mut printed = Vec::new();
    while let Ok(line) = lines.recv_timeout(quiet) {
        let restart = line == RESTART;
        printed.push(line);
        if restart {
            drop(server.stdin.take());
            break;
        }
    }
    let saved = saving
        .join()
        .expect("the save was answered before the server ended");
    let written = std::fs::read_to_string(&config).unwrap();
    drop(server);
    printed.extend(lines.try_iter());
    let _ = std::fs::remove_dir_all(&dir);

    assert!(refused.starts_with("HTTP/1.1 403"), "{refused}");
    assert!(saved.starts_with("HTTP/1.1 200 OK"), "{saved}");

    // Whole, however the server frames it, though the launcher ended the server
    // on the restart line: that line waits for this to have gone out.
    assert!(
        saved.ends_with(&format!(r#"{{"appRestarts":{restarts},"saved":true}}"#)),
        "{saved}"
    );
    assert_eq!(written, contents("new-key"));
    assert_eq!(
        printed.iter().any(|line| line == RESTART),
        restarts,
        "{printed:?}"
    );
    assert!(
        printed.iter().all(|line| !line.contains(CODE)),
        "{printed:?}"
    );
}

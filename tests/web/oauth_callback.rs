//! GitHub OAuth callback failure pages and retry links.

use super::*;

fn oauth_test_config(path: &std::path::Path) -> WebServerConfig {
    initialize_account_database(path).unwrap();

    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.to_path_buf());
    config
}

#[tokio::test]
async fn github_callback_cancelled_shows_retry_page() {
    let path = account_db_path("oauth-cancel");

    let config = oauth_test_config(&path);
    let (base, server) = spawn_web_server(config).await;

    let response = http_client()
        .get(format!("{base}/api/callback?error=access_denied"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 400);

    let content_type = response.headers()["content-type"].to_str().unwrap();
    assert!(content_type.starts_with("text/html"));

    let body = response.text().await.unwrap();
    assert!(body.contains("GitHub sign-in was cancelled."));
    assert!(body.contains("href=\"/api/login\""));

    server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn github_callback_cancelled_from_peer_lobby_has_retry_link() {
    let (github_base, github_server) = spawn_mock_github().await;
    let path = account_db_path("peer-retry");

    let mut config = oauth_test_config(&path);
    config.github_oauth_base_url = Some(github_base.clone());
    config.github_api_base_url = Some(github_base);

    let (base, server) = spawn_web_server(config).await;

    let client = http_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // Start GitHub sign-in from the Peer Lobby.
    let login = client
        .get(format!("{base}/api/login?next=peer-join"))
        .send()
        .await
        .unwrap();

    assert_eq!(login.status(), 302);

    // The return destination must outlive the OAuth state.
    for (cookie_name, expected_age) in [
        ("codetrial_oauth_state", "Max-Age=600"),
        ("codetrial_oauth_return", "Max-Age=3600"),
    ] {
        let cookie = login
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap())
            .find(|value| value.starts_with(&format!("{cookie_name}=")))
            .expect("login must set both OAuth cookies");

        assert!(
            cookie
                .split(';')
                .map(str::trim)
                .any(|attribute| attribute.eq_ignore_ascii_case(expected_age)),
            "{cookie_name} has an unexpected Max-Age: {cookie}"
        );
    }

    let cookies = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|cookie| cookie.to_str().unwrap().split(';').next().unwrap())
        .collect::<Vec<_>>()
        .join("; ");

    // A valid signed return cookie can preserve Peer Lobby for retry,
    // independently of the original OAuth state.
    let state_cookie_header = cookies
        .split("; ")
        .find(|cookie| cookie.starts_with("codetrial_oauth_state="))
        .unwrap();

    // The browser retains the return cookie after the callback clears state.
    let retained_return_cookie = cookies
        .split("; ")
        .find(|cookie| cookie.starts_with("codetrial_oauth_return="))
        .expect("Peer Lobby login must set a return cookie")
        .to_owned();
    let stale_return = signed_cookie(
        "codetrial_oauth_return",
        &format!("{}:/peer-lobby", "a".repeat(32)),
        "session-secret",
    );

    let mismatch = client
        .get(format!("{base}/api/callback?error=access_denied"))
        .header(
            "cookie",
            format!("{state_cookie_header}; codetrial_oauth_return={stale_return}"),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(mismatch.status(), 400);

    let body = mismatch.text().await.unwrap();
    assert!(body.contains("href=\"/api/login?next=peer-join\""));

    // An expired OAuth state does not prevent a retry when the return cookie is
    // correctly signed.
    let expired_token = "b".repeat(32);
    let expired_state = state_cookie_aged(&expired_token, "session-secret", 10 * 60 + 1);

    // A mismatched callback state still fails authentication, but a valid
    // signed return cookie can preserve the retry destination.
    let newer_token = "c".repeat(32);
    let newer_state = state_cookie(&newer_token, "session-secret");
    let newer_return = signed_cookie(
        "codetrial_oauth_return",
        &format!("{newer_token}:/peer-lobby"),
        "session-secret",
    );

    let older_callback = client
        .get(format!("{base}/api/callback?code=unused&state=older-state"))
        .header(
            "cookie",
            format!(
                "codetrial_oauth_state={newer_state}; \
             codetrial_oauth_return={newer_return}"
            ),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(older_callback.status(), 401);

    let body = older_callback.text().await.unwrap();
    assert!(body.contains("href=\"/api/login?next=peer-join\""));

    let matching_return = signed_cookie(
        "codetrial_oauth_return",
        &format!("{expired_token}:/peer-lobby"),
        "session-secret",
    );

    let expired = client
        .get(format!(
            "{base}/api/callback?code=unused&state={expired_token}"
        ))
        .header(
            "cookie",
            format!(
                "codetrial_oauth_state={expired_state}; codetrial_oauth_return={matching_return}"
            ),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(expired.status(), 401);

    let body = expired.text().await.unwrap();
    assert!(body.contains("href=\"/api/login?next=peer-join\""));

    // The user cancels GitHub authorization.
    let callback = client
        .get(format!("{base}/api/callback?error=access_denied"))
        .header("cookie", cookies)
        .send()
        .await
        .unwrap();

    assert_eq!(callback.status(), 400);

    // The failed callback clears OAuth state but preserves the return cookie.
    assert!(clears_state_cookie(callback.headers()));
    assert!(
        !callback
            .headers()
            .get_all("set-cookie")
            .iter()
            .any(|value| value
                .to_str()
                .unwrap()
                .starts_with("codetrial_oauth_return=")),
        "the failed callback must preserve the return cookie"
    );

    let body = callback.text().await.unwrap();

    // Retry must preserve the Peer Lobby destination.
    let retry_path = "/api/login?next=peer-join";
    assert!(body.contains(&format!("href=\"{retry_path}\"")));

    // An explicitly empty callback state is equivalent to a missing state.
    let empty_state = client
        .get(format!("{base}/api/callback?error=access_denied&state="))
        .header("cookie", retained_return_cookie.clone())
        .send()
        .await
        .unwrap();

    assert_eq!(empty_state.status(), 400);

    let body = empty_state.text().await.unwrap();
    assert!(body.contains("href=\"/api/login?next=peer-join\""));

    // Reload the failed callback after the browser has cleared the state
    // cookie.
    let reloaded = client
        .get(format!("{base}/api/callback?error=access_denied"))
        .header("cookie", retained_return_cookie)
        .send()
        .await
        .unwrap();

    assert_eq!(reloaded.status(), 400);

    let reloaded_body = reloaded.text().await.unwrap();
    assert!(
        reloaded_body.contains(&format!("href=\"{retry_path}\"")),
        "reloading the failure page should preserve the Peer Lobby retry link"
    );

    // Retry uses the signed return destination without matching the callback
    // state. An invalid signature must still be rejected.
    for (why, return_cookie, expected_retry) in [
        (
            "signed return cookie without matching callback state",
            signed_cookie(
                "codetrial_oauth_return",
                &format!("{}:/peer-lobby", "d".repeat(32)),
                "session-secret",
            ),
            "/api/login?next=peer-join",
        ),
        (
            "invalid return cookie signature",
            signed_cookie(
                "codetrial_oauth_return",
                &format!("{}:/peer-lobby", "e".repeat(32)),
                "wrong-session-secret",
            ),
            "/api/login",
        ),
    ] {
        let response = client
            .get(format!("{base}/api/callback?error=access_denied"))
            .header("cookie", format!("codetrial_oauth_return={return_cookie}"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 400, "{why}");

        let body = response.text().await.unwrap();
        assert!(
            body.contains(&format!("href=\"{expected_retry}\"")),
            "{why}"
        );
    }

    // The retry link must start another OAuth authorization.
    let retry = client
        .get(format!("{base}{retry_path}"))
        .send()
        .await
        .unwrap();

    assert_eq!(retry.status(), 302);
    assert!(
        retry.headers()["location"]
            .to_str()
            .unwrap()
            .contains("/login/oauth/authorize?")
    );

    // Complete the retry using the fresh OAuth state and cookies.
    let (state, retry_cookies) = oauth_login_pair(retry.headers());

    let success = client
        .get(format!("{base}/api/callback"))
        .query(&[("code", "ok"), ("state", state.as_str())])
        .header("cookie", retry_cookies)
        .send()
        .await
        .unwrap();

    assert_eq!(success.status(), 302);
    assert_eq!(success.headers()["location"], "/peer-lobby");

    server.shutdown().await;
    github_server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn github_callback_sets_session_and_clears_oauth_state() {
    let (github_base, github_server) = spawn_mock_github().await;
    let path = account_db_path("callback-accounts");

    let mut config = oauth_test_config(&path);
    config.github_oauth_base_url = Some(github_base.clone());
    config.github_api_base_url = Some(github_base);

    let (base, server) = spawn_web_server(config).await;

    let client = http_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let assignment =
        "/t/classroom/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7";
    for (return_to, expected) in [
        (assignment, assignment),
        (
            "/t/classroom/delimiter-closer?site=https://teacher.github.io/a;b,c%20d&version=7",
            "/t/classroom/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fa%3Bb%2Cc%20d&version=7",
        ),
        (
            "/t/classroom/delimiter-closer?site=https://teacher.github.io/course&version=7&ref=x",
            "/",
        ),
        ("//attacker.example/t/set/task", "/"),
        (
            "/t/classroom/delimiter-closer?site=https%3A%2F%2Fteacher.github.io&version=7&version=8",
            "/",
        ),
        (
            "/t/classroom/delimiter-closer?site=https%3A%2F%2Fteacher.github.io&version=0",
            "/",
        ),
    ] {
        let login = client
            .get(format!("{base}/api/login"))
            .query(&[("returnTo", return_to)])
            .send()
            .await
            .unwrap();
        assert_eq!(login.status(), 302);
        let (state, cookies) = oauth_login_pair(login.headers());
        let callback = client
            .get(format!("{base}/api/callback"))
            .query(&[("code", "ok"), ("state", state.as_str())])
            .header("cookie", cookies)
            .send()
            .await
            .unwrap();
        assert_eq!(callback.status(), 302);
        assert_eq!(callback.headers()["location"], expected);
    }

    let response = client
        .get(format!("{base}/api/callback?code=ok&state=state-token"))
        .header(
            "cookie",
            format!(
                "codetrial_oauth_state={}",
                state_cookie("state-token", "session-secret")
            ),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 302);
    let cookies = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(cookies.len(), 3);
    assert!(cookies.iter().any(
        |cookie| cookie.starts_with("codetrial_oauth_return=") && cookie.contains("Max-Age=0")
    ));
    let session_cookie = cookies
        .iter()
        .find(|cookie| cookie.starts_with("codetrial_session="))
        .expect("callback should set session cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert!(
        clears_state_cookie(response.headers()),
        "callback should clear oauth state cookie"
    );

    let session = http_client()
        .get(format!("{base}/api/session"))
        .header("cookie", session_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(session["signedIn"], true);
    assert_eq!(session["user"]["login"], "octocat");
    assert_eq!(session["githubSignedIn"], true);

    let issued = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let task_return = client
        .get(format!("{base}/api/callback?code=ok&state=task-state"))
        .header(
            "cookie",
            format!(
                "codetrial_oauth_state={}; codetrial_oauth_return={}",
                signed_cookie(
    "codetrial_oauth_state",
    &format!("{issued}:task-state"),
    "session-secret",
),
signed_cookie(
    "codetrial_oauth_return",
    "task-state:/t/classroom/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7",
    "session-secret",
),
            ),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(task_return.status(), 302);
    assert_eq!(
        task_return.headers()["location"],
        "/t/classroom/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7"
    );
    // Bank ids such as `3sum` start with a digit and still return.
    let digit_return = client
        .get(format!("{base}/api/callback?code=ok&state=digit-state"))
        .header(
            "cookie",
            format!(
                "codetrial_oauth_state={}; codetrial_oauth_return={}",
                signed_cookie(
                    "codetrial_oauth_state",
                    &format!("{issued}:digit-state"),
                    "session-secret",
                ),
                signed_cookie(
                    "codetrial_oauth_return",
                    "digit-state:/t/classroom/3sum",
                    "session-secret",
                ),
            ),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(digit_return.status(), 302);
    assert_eq!(digit_return.headers()["location"], "/t/classroom/3sum");
    let mismatched = client
        .get(format!("{base}/api/callback?code=ok&state=other-state"))
        .header(
            "cookie",
            format!(
                "codetrial_oauth_state={}; codetrial_oauth_return={}",
                signed_cookie(
                    "codetrial_oauth_state",
                    &format!("{issued}:other-state"),
                    "session-secret",
                ),
                signed_cookie(
                    "codetrial_oauth_return",
                    "task-state:/t/classroom/delimiter-closer",
                    "session-secret",
                ),
            ),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(mismatched.status(), 302);
    assert_eq!(mismatched.headers()["location"], "/");

    server.shutdown().await;
    github_server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn github_callback_rate_limit_uses_html_error_page() {
    let path = unique_temp_path("oauth-callback-rate-limit", ".db");

    let config = oauth_test_config(&path);
    let (base, server) = spawn_web_server(config).await;

    let client = http_client();
    let callback = format!("{base}/api/callback?error=access_denied");

    // Requests within the limit still receive the normal cancellation page.
    for attempt in 1..=TOKEN_RATE_LIMIT {
        let response = client.get(&callback).send().await.unwrap();
        assert_eq!(response.status(), 400, "attempt {attempt}");
    }

    let return_cookie = signed_cookie(
        "codetrial_oauth_return",
        &format!("{}:/peer-lobby", "f".repeat(32)),
        "session-secret",
    );

    let blocked = client
        .get(&callback)
        .header("cookie", format!("codetrial_oauth_return={return_cookie}"))
        .send()
        .await
        .unwrap();

    assert_eq!(blocked.status(), 429);
    assert_eq!(blocked.headers()["retry-after"], "60");
    assert_eq!(blocked.headers()["cache-control"], "no-store");
    assert!(
        blocked.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );

    let body = blocked.text().await.unwrap();
    assert!(body.contains("Too many login attempts."));
    assert!(body.contains("href=\"/api/login?next=peer-join\""));

    // An explicitly empty state must preserve the Peer Lobby retry link even
    // when the callback is rate-limited.
    let blocked_empty_state = client
        .get(format!("{callback}&state="))
        .header("cookie", format!("codetrial_oauth_return={return_cookie}"))
        .send()
        .await
        .unwrap();

    assert_eq!(blocked_empty_state.status(), 429);

    let body = blocked_empty_state.text().await.unwrap();
    assert!(body.contains("href=\"/api/login?next=peer-join\""));

    server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn github_callback_upstream_failures_show_html_retry_pages() {
    let path = unique_temp_path("oauth-upstream-failure", ".db");
    let (github_base, github_server) = spawn_mock_github().await;

    let mut config = oauth_test_config(&path);
    config.github_oauth_base_url = Some(github_base.clone());

    // The token endpoint works, but the profile endpoint returns 404.
    config.github_api_base_url = Some(format!("{github_base}/unavailable"));

    let (base, server) = spawn_web_server(config).await;

    let client = http_client();

    // Start one upstream-failure case from a real Peer Lobby login.
    let login = http_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!("{base}/api/login?next=peer-join"))
        .send()
        .await
        .unwrap();

    assert_eq!(login.status(), 302);

    let (peer_state, peer_cookies) = oauth_login_pair(login.headers());

    let profile_state = "profile-error-state";
    let profile_cookies = format!(
        "codetrial_oauth_state={}",
        state_cookie(profile_state, "session-secret")
    );

    for (why, code, callback_state, cookies, expected_message, expected_href) in [
        (
            "token exchange failure",
            "invalid",
            peer_state.as_str(),
            peer_cookies.as_str(),
            "GitHub token exchange failed. Please try again.",
            "href=\"/api/login?next=peer-join\"",
        ),
        (
            "profile API failure",
            "ok",
            profile_state,
            profile_cookies.as_str(),
            "GitHub profile request failed. Please try again.",
            "href=\"/api/login\"",
        ),
    ] {
        let response = client
            .get(format!("{base}/api/callback"))
            .query(&[("code", code), ("state", callback_state)])
            .header("cookie", cookies)
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 502, "{why}");
        assert!(
            response.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("text/html"),
            "{why}"
        );
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert!(clears_state_cookie(response.headers()), "{why}");

        let body = response.text().await.unwrap();
        assert!(body.contains("GitHub sign-in failed"), "{why}");
        assert!(
            body.contains(&format!("<p>{expected_message}</p>")),
            "{why}"
        );
        assert!(body.contains("Try signing in again"), "{why}");
        assert!(body.contains(expected_href), "{why}");
    }

    server.shutdown().await;
    github_server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn github_callback_session_creation_failure_shows_html_retry_page() {
    let path = unique_temp_path("oauth-session-failure", ".db");
    let (github_base, github_server) = spawn_mock_github().await;

    let mut config = oauth_test_config(&path);
    config.github_oauth_base_url = Some(github_base.clone());
    config.github_api_base_url = Some(github_base);

    // Force session creation to fail without changing production code.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER refuse_session BEFORE INSERT ON sessions
             BEGIN SELECT RAISE(ABORT, 'session refused'); END;",
        )
        .unwrap();

    let (base, server) = spawn_web_server(config).await;

    let state = "session-failure-state";
    let cookie = state_cookie(state, "session-secret");

    let response = http_client()
        .get(format!("{base}/api/callback"))
        .query(&[("code", "ok"), ("state", state)])
        .header("cookie", format!("codetrial_oauth_state={cookie}"))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 500);
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(clears_state_cookie(response.headers()));

    // A failed sign-in must not issue a session cookie.
    assert!(
        !response
            .headers()
            .get_all("set-cookie")
            .iter()
            .any(|value| value.to_str().unwrap().starts_with("codetrial_session=")),
        "failed session creation must not issue a session cookie"
    );

    let body = response.text().await.unwrap();
    assert!(body.contains("Could not create account session. Please try again."));
    assert!(body.contains("href=\"/api/login\""));

    server.shutdown().await;
    github_server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn github_login_return_cookie_fits_chromium_limit() {
    let path = account_db_path("oauth-return-cookie-size");
    let mut config = oauth_test_config(&path);
    config.github_client_id = Some("c".repeat(20));
    config.production = true;
    let (base, server) = spawn_web_server(config).await;
    let client = http_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let prefix = "/t/classroom/3sum?site=https%3A%2F%2Fteacher.github.io%2F";
    let suffix = "&version=7";
    for destination_size in [2880, 3997, 3998] {
        let site_path = "a".repeat(destination_size - prefix.len() - suffix.len());
        let destination = format!("{prefix}{site_path}{suffix}");
        let expected_path = if destination_size == 3998 {
            "/"
        } else {
            destination.as_str()
        };
        let response = client
            .get(format!("{base}/api/login"))
            .query(&[("returnTo", &destination)])
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 302);
        // Count the upstream HTTP/1.1 response nginx has to buffer.
        let header_bytes = "HTTP/1.1 302 Found\r\n\r\n".len()
            + response
                .headers()
                .iter()
                .map(|(name, value)| name.as_str().len() + ": \r\n".len() + value.as_bytes().len())
                .sum::<usize>();
        assert_eq!(
            header_bytes <= 4096,
            destination_size != 3997,
            "{destination_size}-byte destination produced {header_bytes} bytes of response headers"
        );

        let pair = response
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|value| value.to_str().unwrap())
            .find(|cookie| cookie.split("; ").any(|part| part == "Max-Age=3600"))
            .expect("login must set a return cookie")
            .split(';')
            .next()
            .unwrap();
        let (name, value) = pair.split_once('=').unwrap();
        assert_eq!(name, "codetrial_oauth_return");
        let (payload, _) = value.rsplit_once('.').unwrap();
        assert!(payload.ends_with(&format!(":{expected_path}")));

        let cookie_size = name.len() + value.len();
        assert_eq!(cookie_size, expected_path.len() + 99);
        assert!(
            cookie_size <= 4096,
            "{destination_size}-byte destination produced a {cookie_size}-byte cookie"
        );
    }

    server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn github_oauth_return_cookie_keeps_legacy_wire_format_and_survives_upgrade() {
    let (github_base, github_server) = spawn_mock_github().await;
    let path = account_db_path("oauth-legacy-return-cookie");
    let mut config = oauth_test_config(&path);
    config.github_oauth_base_url = Some(github_base.clone());
    config.github_api_base_url = Some(github_base);
    config.production = true;
    let (base, server) = spawn_web_server(config).await;
    let client = http_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let prefix = "/t/classroom/3sum?site=https%3A%2F%2Fteacher.github.io%2F";
    let suffix = "&version=7";
    let site_path = "a".repeat(3996 - prefix.len() - suffix.len());
    let legacy_destination = format!("{prefix}{site_path}{suffix}");
    let destination = format!("{prefix}{site_path}a{suffix}");
    let legacy_name = "codetrial_oauth_return";
    let legacy_value = signed_cookie(
        legacy_name,
        &format!("{}:{legacy_destination}", "a".repeat(32)),
        "session-secret",
    );
    let legacy_pair = format!("{legacy_name}={legacy_value}");
    let login = client
        .get(format!("{base}/api/login"))
        .query(&[("returnTo", &destination)])
        .header("cookie", &legacy_pair)
        .send()
        .await
        .unwrap();

    assert_eq!(login.status(), 302);
    let authorize = reqwest::Url::parse(login.headers()["location"].to_str().unwrap()).unwrap();
    let state = authorize
        .query_pairs()
        .find(|(key, _)| key == "state")
        .unwrap()
        .1;
    let cookies: Vec<_> = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|header| header.to_str().unwrap())
        .collect();
    assert_eq!(cookies.len(), 2, "login must not add a migration header");
    let new_return = cookies
        .iter()
        .find(|cookie| cookie.starts_with("codetrial_oauth_return="))
        .unwrap();
    assert!(new_return.ends_with("; Path=/; HttpOnly; SameSite=Lax; Secure"));
    let new_pair = new_return.split(';').next().unwrap();
    let expected = signed_cookie(
        legacy_name,
        &format!("{state}:{destination}"),
        "session-secret",
    );
    assert_eq!(new_pair, format!("{legacy_name}={expected}"));

    let legacy_state = "a".repeat(32);
    let legacy_state_pair = format!(
        "codetrial_oauth_state={}",
        state_cookie(&legacy_state, "session-secret")
    );
    let new_state_pair = cookies
        .iter()
        .find(|cookie| cookie.starts_with("codetrial_oauth_state="))
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    for (first, second, state, state_pair, destination) in [
        (
            legacy_pair.as_str(),
            new_pair,
            state.as_ref(),
            new_state_pair,
            destination.as_str(),
        ),
        (
            new_pair,
            legacy_pair.as_str(),
            legacy_state.as_str(),
            legacy_state_pair.as_str(),
            legacy_destination.as_str(),
        ),
    ] {
        let mut jar = std::collections::BTreeMap::new();
        for pair in [first, second, state_pair] {
            jar.insert(pair.split_once('=').unwrap().0, pair);
        }
        assert_eq!(jar.len(), 2);
        let header = jar.values().copied().collect::<Vec<_>>().join("; ");
        assert!(header.len() + "Cookie: \r\n".len() <= 8192);
        assert!(header.len() + "Cookie: ; \r\n".len() + first.len() > 8192);

        let callback = client
            .get(format!("{base}/api/callback"))
            .query(&[("code", "ok"), ("state", state)])
            .header("cookie", header)
            .send()
            .await
            .unwrap();
        assert_eq!(callback.status(), 302);
        assert_eq!(callback.headers()["location"], destination);
        let header_bytes = "HTTP/1.1 302 Found\r\n\r\n".len()
            + callback
                .headers()
                .iter()
                .map(|(name, value)| name.as_str().len() + ": \r\n".len() + value.as_bytes().len())
                .sum::<usize>();
        assert!((4097..=8192).contains(&header_bytes));
    }

    server.shutdown().await;
    github_server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn github_callback_cancelled_from_task_preserves_retry_destination() {
    let (github_base, github_server) = spawn_mock_github().await;
    let path = unique_temp_path("oauth-task-retry", ".db");

    let mut config = oauth_test_config(&path);
    config.github_oauth_base_url = Some(github_base.clone());
    config.github_api_base_url = Some(github_base);

    let (base, server) = spawn_web_server(config).await;

    let client = http_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let task_path = "/t/classroom/3sum";

    // Start sign-in from the task page.
    let login = client
        .get(format!("{base}/api/login"))
        .query(&[("returnTo", task_path)])
        .send()
        .await
        .unwrap();

    assert_eq!(login.status(), 302);

    let cookies = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|cookie| cookie.to_str().unwrap().split(';').next().unwrap())
        .collect::<Vec<_>>()
        .join("; ");

    // The user cancels GitHub authorization.
    let cancelled = client
        .get(format!("{base}/api/callback?error=access_denied"))
        .header("cookie", cookies)
        .send()
        .await
        .unwrap();

    assert_eq!(cancelled.status(), 400);
    assert!(
        cancelled.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );

    let body = cancelled.text().await.unwrap();

    // Retry must preserve the original task destination.
    let retry_path = "/api/login?returnTo=%2Ft%2Fclassroom%2F3sum";
    assert!(body.contains(&format!("href=\"{retry_path}\"")));

    // Retry starts a fresh OAuth authorization.
    let retry = client
        .get(format!("{base}{retry_path}"))
        .send()
        .await
        .unwrap();

    assert_eq!(retry.status(), 302);

    let (fresh_state, retry_cookies) = oauth_login_pair(retry.headers());

    // Complete the new login and return to the original task.
    let success = client
        .get(format!("{base}/api/callback"))
        .query(&[("code", "ok"), ("state", fresh_state.as_str())])
        .header("cookie", retry_cookies)
        .send()
        .await
        .unwrap();

    assert_eq!(success.status(), 302);
    assert_eq!(success.headers()["location"], task_path);

    // Preserve query parameters, but reject unsafe return destinations.
    for (return_to, expected_retry) in [
        (
            "/t/classroom/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse%22%26&version=7",
            "/api/login?returnTo=%2Ft%2Fclassroom%2Fdelimiter-closer%3Fsite%3Dhttps%253A%252F%252Fteacher.github.io%252Fcourse%2522%2526%26version%3D7",
        ),
        (
            "/t/classroom/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7",
            "/api/login?returnTo=%2Ft%2Fclassroom%2Fdelimiter-closer%3Fsite%3Dhttps%253A%252F%252Fteacher.github.io%252Fcourse%26version%3D7",
        ),
        ("https://evil.example", "/api/login"),
    ] {
        let login = client
            .get(format!("{base}/api/login"))
            .query(&[("returnTo", return_to)])
            .send()
            .await
            .unwrap();

        assert_eq!(login.status(), 302);

        let cookies = login
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|cookie| cookie.to_str().unwrap().split(';').next().unwrap())
            .collect::<Vec<_>>()
            .join("; ");

        let cancelled = client
            .get(format!("{base}/api/callback?error=access_denied"))
            .header("cookie", cookies)
            .send()
            .await
            .unwrap();

        assert_eq!(cancelled.status(), 400);

        let body = cancelled.text().await.unwrap();
        assert!(
            body.contains(&format!("href=\"{expected_retry}\"")),
            "unexpected retry destination for {return_to}"
        );
    }

    server.shutdown().await;
    github_server.shutdown().await;
    remove_database(path).await;
}

//! The router's shape, and what does not belong to another file here.
//!
//! Split out of `tests/web.rs`, which is still the test target: cargo
//! discovers only `tests/*.rs`, so this compiles as a module of that one
//! binary rather than relinking the crate for a file of its own.

use super::*;

/// The callback answers 502 on two separate legs -- a refused token exchange
/// and a refused profile request -- so the status alone cannot say which one
/// ran. The message is the only thing that tells them apart, which is why this
/// pins the text and not just the code: without it, a token exchange that had
/// stopped working entirely would still satisfy every 502 assertion in this
/// file, because the profile leg it never reaches is the one they were really
/// describing.
///
/// The stub refuses any `code` but the fixture, so asking for a different one
/// is enough to fail the exchange while leaving `/user` perfectly healthy.
#[tokio::test]
async fn a_refused_code_exchange_names_the_token_leg_not_the_profile_leg() {
    let (github_base, github_server) = spawn_mock_github().await;
    let path = std::env::temp_dir().join(format!(
        "codetrial-callback-bad-code-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    config.github_oauth_base_url = Some(github_base.clone());
    config.github_api_base_url = Some(github_base);

    // The schema is created once at startup, not per request, so a caller that
    // builds the router directly has to do what `main` does.
    initialize_account_database(&path).unwrap();
    let (base, server) = spawn_web_server(config).await;
    let client = http_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let response = client
        .get(format!("{base}/api/callback?code=stale&state=state-token"))
        .header(
            "cookie",
            format!(
                "codetrial_oauth_state={}",
                signed_cookie("state-token", "session-secret")
            ),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 502);
    let body = response.json::<Value>().await.unwrap();
    assert_eq!(
        body["error"], "GitHub token exchange failed.",
        "a refused code fails the exchange, not the profile request"
    );

    server.shutdown().await;
    github_server.shutdown().await;
    remove_database(path).await;
}

/// The callback only accepts the login it started.
///
/// `/api/login` mints a state token, signs it into a cookie, and puts the same
/// value in the authorize URL; the callback is what compares them, and that
/// comparison is the only thing standing between a candidate and a session
/// somebody else's browser was made to create. Nothing asserted it: the whole
/// cookie lookup and the equality below it were deleted and every test in this
/// file still passed.
///
/// The happy path at the end is the other half. It is the only place the two
/// ends are checked against each other rather than against a fixture, so a
/// callback that demanded a state nothing mints would be caught here rather
/// than in a browser.
#[tokio::test]
async fn the_oauth_callback_requires_the_state_it_minted() {
    let (github_base, github_server) = spawn_mock_github().await;
    let path = account_db_path("oauth-state");
    initialize_account_database(&path).unwrap();
    let mut config = web_config();
    config.github_client_id = Some(MOCK_GITHUB_CLIENT_ID.to_string());
    config.github_client_secret = Some(MOCK_GITHUB_CLIENT_SECRET.to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    config.github_oauth_base_url = Some(github_base.clone());
    config.github_api_base_url = Some(github_base);
    let (base, server) = spawn_web_server(config).await;
    let client = http_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // The pair the server itself just handed out, rather than a fixture: what
    // is under test is that these two are compared, so both have to come from
    // the same `/api/login`.
    let started = client
        .get(format!("{base}/api/login"))
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), 302);
    let minted = query_parameter(
        started.headers().get("location").unwrap().to_str().unwrap(),
        "state",
    )
    .expect("the authorize URL carries a state token");
    let state_cookie = started
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|cookie| cookie.starts_with("codetrial_oauth_state="))
        .expect("login should set the oauth state cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let callback = |cookie: Option<String>, state: &str| {
        let client = client.clone();
        let url = format!("{base}/api/callback?code={MOCK_GITHUB_CODE}&state={state}");
        async move {
            let request = client.get(url);
            match cookie {
                Some(cookie) => request.header("cookie", cookie).send().await.unwrap(),
                None => request.send().await.unwrap(),
            }
        }
    };

    for (why, cookie, state, message) in [
        (
            "no state cookie at all",
            None,
            minted.clone(),
            "Login state is missing or expired.",
        ),
        (
            "a state cookie nobody signed",
            Some(format!("codetrial_oauth_state={minted}")),
            minted.clone(),
            "Login state is missing or expired.",
        ),
        (
            "a state cookie signed with another secret",
            Some(format!(
                "codetrial_oauth_state={}",
                signed_cookie(&minted, "another-session-secret")
            )),
            minted.clone(),
            "Login state is missing or expired.",
        ),
        (
            "a state parameter this browser was never given",
            Some(state_cookie.clone()),
            "state-from-somewhere-else".to_string(),
            "Login state did not match.",
        ),
    ] {
        let refused = callback(cookie, &state).await;
        assert_eq!(refused.status(), 401, "{why}");
        assert_eq!(
            refused.json::<Value>().await.unwrap()["error"],
            message,
            "{why}"
        );
    }

    // A code with no state, and a state with no code, are the client's mistake
    // rather than a forgery, and answer as one.
    for query in ["code=ok", "state=whatever", ""] {
        let incomplete = client
            .get(format!("{base}/api/callback?{query}"))
            .header("cookie", &state_cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(incomplete.status(), 400, "{query:?}");
    }

    // And the pair this server minted completes the sign-in, so the refusals
    // above are a comparison rather than a callback that refuses everyone.
    let accepted = callback(Some(state_cookie.clone()), &minted).await;
    assert_eq!(accepted.status(), 302);
    assert!(
        accepted
            .headers()
            .get_all("set-cookie")
            .iter()
            .any(|value| value.to_str().unwrap().starts_with("codetrial_session=")),
        "the matched state is what a session is issued against"
    );

    // The same pair, percent-encoded. Both parameters are decoded before they
    // are used -- `state` to compare against the cookie, `code` to post to
    // GitHub -- and nothing else in this suite reaches that branch, because
    // every value this server mints is already unreserved and is echoed back
    // verbatim. GitHub is under no obligation to echo it back that way.
    let percent_first = |value: &str| format!("%{:02X}{}", value.as_bytes()[0], &value[1..]);
    let encoded = client
        .get(format!(
            "{base}/api/callback?code={}&state={}",
            percent_first(MOCK_GITHUB_CODE),
            percent_first(&minted)
        ))
        .header("cookie", &state_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(
        encoded.status(),
        302,
        "an encoded callback has to decode to the code and the state that were minted"
    );

    server.shutdown().await;
    github_server.shutdown().await;
    remove_database(path).await;
}

/// The same endpoint on a server that records nothing, so the cap is the range
/// the endpoint has always offered rather than a number the lobby invents.
#[tokio::test]
async fn a_server_that_does_not_record_caps_nothing_beyond_the_range() {
    let (base, server) = spawn_web_server(web_config()).await;

    let session = http_client()
        .get(format!("{base}/api/session"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();

    assert_eq!(session["maxDurationMin"], MAX_DURATION_MIN);
    server.shutdown().await;
}

#[test]
fn router_routes_match_a_fixed_allowlist() {
    let routes = declared_routes();
    let mut allowed = vec![
        "/healthz",
        "/runtime-config.js",
        "/api/token",
        "/api/task-reviews/{room}",
        "/api/task-sets/{set_id}/tasks/{task_id}",
        "/api/task-sets/{set_id}/tasks/{task_id}/attempts",
        "/api/task-sets/{set_id}/unlock",
        "/api/observer-token",
        "/api/login",
        "/api/callback",
        "/api/github/device",
        "/api/github/device/poll",
        "/api/session",
        "/api/logout",
        "/api/recordings",
        "/api/recordings/{id}",
        "/api/recordings/{id}/events",
        "/api/reports",
        "/api/reports/{id}",
        "/api/interviews",
        "/api/interviews/{id}/consent",
        "/api/interviews/{id}/recording",
        "/api/interviews/{id}/end",
        "/api/interviews/{id}/events",
        "/api/interviews/{id}/snapshot",
        "/api/recording/replay",
        "/api/recording/webhook",
    ];
    allowed.sort_unstable();

    assert_eq!(
        routes, allowed,
        "the route list changed; a new one has to be added here deliberately, and none of them \
         may accept media"
    );
}

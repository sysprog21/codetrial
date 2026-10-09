//! Sign-in, the session cookie, and the reports an account owns.
//!
//! Split out of `tests/web.rs`, which is still the test target: cargo
//! discovers only `tests/*.rs`, so this compiles as a module of that one
//! binary rather than relinking the crate for a file of its own.

use super::*;

#[tokio::test]
async fn account_database_schema_creates_login_tables() {
    let path = std::env::temp_dir().join(format!(
        "codetrial-accounts-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    initialize_account_database(&path).unwrap();

    let connection = rusqlite::Connection::open(&path).unwrap();
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap();
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(tables.contains(&"users".to_string()));
    assert!(tables.contains(&"sessions".to_string()));
    assert!(tables.contains(&"reports".to_string()));

    for (table, expected) in [
        (
            "users",
            [
                "github_id INTEGER NOT NULL UNIQUE",
                "login TEXT NOT NULL",
                "created_at INTEGER NOT NULL",
                "updated_at INTEGER NOT NULL",
            ]
            .as_slice(),
        ),
        (
            "sessions",
            [
                "user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE",
                "expires_at INTEGER NOT NULL",
                "created_at INTEGER NOT NULL",
            ]
            .as_slice(),
        ),
        (
            "reports",
            [
                "user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE",
                "problem_id TEXT NOT NULL",
                "payload TEXT NOT NULL",
                "created_at INTEGER NOT NULL",
                "updated_at INTEGER NOT NULL",
            ]
            .as_slice(),
        ),
    ] {
        let sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        for snippet in expected {
            assert!(
                sql.contains(snippet),
                "{table} schema should contain {snippet}, got {sql}"
            );
        }
    }

    drop(statement);
    drop(connection);
    remove_database(path).await;
}

#[tokio::test]
async fn account_routes_record_interviewee_github_login() {
    let (base, server, path, client) = account_server("record-login").await;

    let login = client
        .get(format!("{base}/api/login"))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 400);
    assert_eq!(
        login.json::<Value>().await.unwrap()["error"],
        "Submit a GitHub username to start an interview."
    );

    let invalid = client
        .post(format!("{base}/api/login"))
        .json(&json!({"login":"-bad-"}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 400);

    let session_cookie = record_login(&client, &base, "@Octo-Cat").await;

    // No OAuth app is configured, so there is no code to exchange and the
    // callback must say so instead of posting empty credentials to GitHub.
    let callback = client
        .get(format!("{base}/api/callback?code=x&state=y"))
        .send()
        .await
        .unwrap();
    assert_eq!(callback.status(), 400);

    let session = client
        .get(format!("{base}/api/session"))
        .send()
        .await
        .unwrap();
    assert_eq!(session.status(), 200);

    assert_eq!(
        session.json::<Value>().await.unwrap(),
        json!({"signedIn": false, "loginRequired": true, "deviceLogin": false, "maxDurationMin": MAX_DURATION_MIN})
    );

    let signed_in = client
        .get(format!("{base}/api/session"))
        .header("cookie", session_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(signed_in["signedIn"], true);
    assert_eq!(signed_in["user"]["login"], "octo-cat");

    let reports = client
        .get(format!("{base}/api/reports"))
        .send()
        .await
        .unwrap();
    assert_eq!(reports.status(), 401);
    let save_report = client
        .post(format!("{base}/api/reports"))
        .json(&json!({"problemId":"two-sum"}))
        .send()
        .await
        .unwrap();
    assert_eq!(save_report.status(), 401);
    let delete_reports = client
        .delete(format!("{base}/api/reports"))
        .send()
        .await
        .unwrap();
    assert_eq!(delete_reports.status(), 401);

    server.shutdown().await;
    remove_database(path).await;
}

/// A typed handle is not proof of anything, so it must not be the key an
/// account is found by. If it were, typing a name someone else used would hand
/// over their interview history, and nothing stops anyone typing any name.
#[tokio::test]
async fn a_claimed_handle_does_not_reach_an_earlier_candidates_reports() {
    let (base, server, path, client) = account_server("claimed-handle").await;

    let first = record_login(&client, &base, "octocat").await;
    let saved = client
        .post(format!("{base}/api/reports"))
        .header("cookie", &first)
        .json(&json!({"id":"report-one","problemId":"two-sum","score":8}))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);

    let second = record_login(&client, &base, "octocat").await;
    assert_ne!(first, second, "each recorded login is its own account");

    let reports = client
        .get(format!("{base}/api/reports"))
        .header("cookie", &second)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        reports["reports"].as_array().unwrap().len(),
        0,
        "claiming a handle must not inherit the reports filed under it"
    );

    let mine = client
        .get(format!("{base}/api/reports"))
        .header("cookie", &first)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(mine["reports"][0]["id"], "report-one");

    server.shutdown().await;
    remove_database(path).await;
}

/// The other half of the fail-closed rule, and the one an operator actually
/// reaches: the file opens fine, but a rolled-back binary finds a schema
/// version from the future and refuses to migrate it. That arm has to answer
/// the same way as an unopenable path, because "I cannot read this database" is
/// not the same statement as "this server has no accounts".
#[tokio::test]
async fn an_unmigratable_account_database_refuses_rather_than_disabling_login() {
    let path = account_db_path("unmigratable");
    initialize_account_database(&path).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(&format!(
            "PRAGMA user_version = {}",
            codetrial::web::ACCOUNT_SCHEMA_VERSION + 1
        ))
        .unwrap();
    let mut config = web_config();
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    let (base, server) = spawn_web_server(config).await;
    let client = http_client();

    let session = client
        .get(format!("{base}/api/session"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        session,
        json!({"signedIn": false, "loginRequired": true, "deviceLogin": false, "maxDurationMin": MAX_DURATION_MIN}),
        "a database this binary cannot migrate still requires a login"
    );

    let token = client
        .post(format!("{base}/api/token"))
        .json(&json!({"problemId":"two-sum","durationMin":45}))
        .send()
        .await
        .unwrap();
    assert_eq!(token.status(), 503);

    server.shutdown().await;
    remove_database(path).await;
}

/// A database that will not open must not read as "this server has no
/// accounts". That would drop the sign-in gate over a bad path: the browser
/// would be told login is optional and let anyone start an interview.
#[tokio::test]
async fn an_unopenable_account_database_refuses_rather_than_disabling_login() {
    // A directory is a path SQLite cannot open as a database file.
    let path = account_db_path("unopenable");
    fs::create_dir_all(&path).unwrap();
    let mut config = web_config();
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    let (base, server) = spawn_web_server(config).await;
    let client = http_client();

    let session = client
        .get(format!("{base}/api/session"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        session,
        json!({"signedIn": false, "loginRequired": true, "deviceLogin": false, "maxDurationMin": MAX_DURATION_MIN}),
        "a broken database still requires a login; it cannot serve one"
    );

    let token = client
        .post(format!("{base}/api/token"))
        .json(&json!({"problemId":"two-sum","durationMin":45}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        token.status(),
        503,
        "unavailable, not 404: the operator has a broken database, not an unconfigured one"
    );

    let login = client
        .post(format!("{base}/api/login"))
        .json(&json!({"login":"octocat"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 503);
    server.shutdown().await;
    fs::remove_dir_all(path).unwrap();

    // The other answer, so "503 rather than 404" above is a distinction this
    // server really draws rather than the only status it knows. A deployment
    // with no cookie secret and no database has nothing broken: it never
    // offered accounts, so the browser is told login is not on offer and the
    // session row says so too.
    let (base, server) = spawn_web_server(web_config()).await;
    let client = http_client();

    let token = client
        .post(format!("{base}/api/token"))
        .json(&json!({"problemId":"two-sum","durationMin":45}))
        .send()
        .await
        .unwrap();
    assert_eq!(token.status(), 404);
    assert_eq!(
        token.json::<Value>().await.unwrap()["error"],
        "GitHub username recording is not configured."
    );

    let session = client
        .get(format!("{base}/api/session"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        session,
        json!({"signedIn": false, "loginRequired": false, "deviceLogin": false, "maxDurationMin": MAX_DURATION_MIN}),
        "nothing to sign in to, and the lobby has to be told so"
    );

    server.shutdown().await;
}

/// `/api/token` needs a session, and this is where sessions come from, so an
/// unlimited login endpoint just moves the flood one door back and grows the
/// account database while it is at it.
#[tokio::test]
async fn login_endpoint_rate_limits_a_noisy_client() {
    let (base, server, path, client) = account_server("login-rate-limit").await;
    let url = format!("{base}/api/login");

    for attempt in 1..=TOKEN_RATE_LIMIT {
        let response = client
            .post(&url)
            .json(&json!({"login":"octocat"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "attempt {attempt} should pass");
    }

    let blocked = client
        .post(&url)
        .json(&json!({"login":"octocat"}))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 429);
    assert_eq!(blocked.headers().get("retry-after").unwrap(), "60");

    server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn login_rejects_a_body_that_is_not_json() {
    let (base, server, path, client) = account_server("login-malformed").await;

    let malformed = client
        .post(format!("{base}/api/login"))
        .header("content-type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 400);

    let oversize = client
        .post(format!("{base}/api/login"))
        .body(vec![b'a'; MAX_BODY_BYTES + 1])
        .send()
        .await
        .unwrap();
    assert_eq!(oversize.status(), 413);

    for handle in ["", "-bad-", "bad name", &"a".repeat(40)] {
        let rejected = client
            .post(format!("{base}/api/login"))
            .json(&json!({ "login": handle }))
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), 400, "{handle:?} is not a GitHub handle");
    }

    server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn github_callback_sets_session_and_clears_oauth_state() {
    let (github_base, github_server) = spawn_mock_github().await;
    let path = std::env::temp_dir().join(format!(
        "codetrial-callback-accounts-{}-{}.db",
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
        let authorize = reqwest::Url::parse(login.headers()["location"].to_str().unwrap()).unwrap();
        let state = authorize
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let cookies = login
            .headers()
            .get_all("set-cookie")
            .iter()
            .map(|cookie| cookie.to_str().unwrap().split(';').next().unwrap())
            .collect::<Vec<_>>()
            .join("; ");
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
                signed_cookie("state-token", "session-secret")
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
        cookies
            .iter()
            .any(|cookie| cookie.starts_with("codetrial_oauth_state=; Max-Age=0")),
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

    let task_return = client
        .get(format!("{base}/api/callback?code=ok&state=task-state"))
        .header(
            "cookie",
            format!(
                "codetrial_oauth_state={}; codetrial_oauth_return={}",
                signed_cookie("task-state", "session-secret"),
                signed_cookie("task-state:/t/classroom/delimiter-closer?site=https%3A%2F%2Fteacher.github.io%2Fcourse&version=7", "session-secret")
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
                signed_cookie("digit-state", "session-secret"),
                signed_cookie("digit-state:/t/classroom/3sum", "session-secret")
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
                signed_cookie("other-state", "session-secret"),
                signed_cookie("task-state:/t/classroom/delimiter-closer", "session-secret")
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

/// A cookie nobody signed is not a session.
///
/// `sessions.id` holds a digest of the cookie payload, so the payload is the
/// whole credential once the signature stops being checked, and it is a value
/// the server itself hands out. Every other test in this file mints its cookie
/// with `signed_cookie`, so a server that ignored the signature entirely
/// answered all of them exactly as it does now; that was measured against
/// `verified_cookie_value` with its `verify_hs256` deleted, and nothing in the
/// whole crate's suite failed.
///
/// The routes are two on purpose. `/api/session` is what the page believes
/// about who it is talking to, and `/api/token` is a live LiveKit credential,
/// so a forged cookie that reached either would be a different kind of loss.
#[tokio::test]
async fn a_forged_session_cookie_is_not_a_session() {
    let (config, cookie, db_path) = signed_in_web_config("forged-session");
    let (base, server) = spawn_web_server(config).await;
    let client = http_client();

    // The payload out of the genuine cookie, so every forgery below names a
    // session row that really exists and only the signature can refuse it.
    let payload = cookie
        .split_once('=')
        .expect("the cookie is name=value")
        .1
        .rsplit_once('.')
        .expect("the value is payload.signature")
        .0
        .to_string();

    for (why, value) in [
        ("no signature at all", payload.clone()),
        ("an empty signature", format!("{payload}.")),
        ("a signature that is not base64", format!("{payload}.zzz")),
        (
            "a signature from another secret",
            signed_cookie(&payload, "another-session-secret"),
        ),
    ] {
        let forged = format!("codetrial_session={value}");
        let session = client
            .get(format!("{base}/api/session"))
            .header("cookie", &forged)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(session["signedIn"], false, "{why}");
        assert!(session.get("user").is_none(), "{why} named an account");

        let token = client
            .post(format!("{base}/api/token"))
            .header("cookie", &forged)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(token.status(), 401, "{why} bought a LiveKit credential");
    }

    // And the cookie the server signed still works, so the four refusals are a
    // check rather than a server that refuses every cookie.
    let signed_in = client
        .get(format!("{base}/api/session"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(signed_in["signedIn"], true);
    assert_eq!(signed_in["user"]["login"], "one");

    server.shutdown().await;
    remove_database(db_path).await;
}

/// The rate limit is keyed by address. If anonymous callers could spend it, a
/// stranger on the same NAT could lock the signed-in candidate out of their own
/// interview without ever holding a session.
#[tokio::test]
async fn anonymous_requests_cannot_spend_the_signed_in_budget() {
    let path = std::env::temp_dir().join(format!(
        "codetrial-token-budget-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    initialize_account_database(&path).unwrap();
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (1, 101, 'one', 1, 1)",
                [],
            )
            .unwrap();
        insert_session(&connection, "session-one", 1);
    }

    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    let (base, server) = spawn_web_server(config).await;
    let url = format!("{base}/api/token");
    let client = http_client();

    // Twice the whole budget, all of it unauthenticated.
    for _ in 0..(TOKEN_RATE_LIMIT * 2) {
        let refused = client.post(&url).json(&json!({})).send().await.unwrap();
        assert_eq!(refused.status(), 401);
    }

    let signed_in = client
        .post(&url)
        .header(
            "cookie",
            format!(
                "codetrial_session={}",
                signed_cookie("session-one", "session-secret")
            ),
        )
        .json(&json!({}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        signed_in.status(),
        200,
        "the candidate's budget was spent by people who never signed in"
    );

    server.shutdown().await;
    remove_database(path).await;
}

#[test]
fn github_oauth_config_is_optional_for_recorded_login() {
    let mut config = web_config();
    assert!(login_config(&config).is_none());

    config.github_client_id = Some("client".to_string());
    config.db_path = Some(Path::new("accounts.db").to_path_buf());
    assert!(
        login_config(&config).is_none(),
        "a database with no cookie secret cannot hold a session"
    );

    config.session_secret = Some("secret".to_string());
    assert!(
        login_config(&config).is_some(),
        "a cookie secret and a database are all a recorded login needs"
    );
}

#[tokio::test]
async fn account_reports_are_scoped_to_the_signed_in_user() {
    let path = std::env::temp_dir().join(format!(
        "codetrial-route-accounts-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    initialize_account_database(&path).unwrap();
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute(
            "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (1, 101, 'one', 1, 1)",
            [],
        ).unwrap();
        connection.execute(
            "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (2, 202, 'two', 1, 1)",
            [],
        ).unwrap();
        insert_session(&connection, "session-one", 1);
        insert_session(&connection, "session-two", 2);
        connection.execute(
            "INSERT INTO reports (id, user_id, problem_id, payload, created_at, updated_at) VALUES ('report-one', 1, 'two-sum', '{\"problemId\":\"two-sum\",\"score\":8}', 1, 1)",
            [],
        ).unwrap();
        connection.execute(
            "INSERT INTO reports (id, user_id, problem_id, payload, created_at, updated_at) VALUES ('report-two', 2, 'merge-intervals', '{\"problemId\":\"merge-intervals\",\"score\":4}', 1, 1)",
            [],
        ).unwrap();
    }

    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    let (base, server) = spawn_web_server(config).await;
    let client = http_client();
    let user_one_cookie = format!(
        "codetrial_session={}",
        signed_cookie("session-one", "session-secret")
    );
    let user_two_cookie = format!(
        "codetrial_session={}",
        signed_cookie("session-two", "session-secret")
    );

    let session = client
        .get(format!("{base}/api/session"))
        .header("cookie", &user_one_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(session["signedIn"], true);
    assert_eq!(session["user"]["login"], "one");

    let reports = client
        .get(format!("{base}/api/reports"))
        .header("cookie", &user_one_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(reports["reports"].as_array().unwrap().len(), 1);
    assert_eq!(reports["reports"][0]["id"], "report-one");

    let saved = client
        .post(format!("{base}/api/reports"))
        .header("cookie", &user_one_cookie)
        .json(&json!({"id":"new-report","problemId":"three-sum","score":9}))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);
    assert_eq!(saved.json::<Value>().await.unwrap()["id"], "new-report");

    let blocked = client
        .post(format!("{base}/api/reports"))
        .header("cookie", &user_two_cookie)
        .json(&json!({"id":"new-report","problemId":"three-sum","score":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 409);

    // The id is the client's own choice, so the route has to take one that only
    // survives the path percent-encoded.
    let saved = client
        .post(format!("{base}/api/reports"))
        .header("cookie", &user_one_cookie)
        .json(&json!({"id":"one/off try","problemId":"two-sum","score":2}))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);
    let delete_one = |cookie: &str, id: &str| {
        client
            .delete(format!("{base}/api/reports/{id}"))
            .header("cookie", cookie)
            .send()
    };
    let theirs = delete_one(&user_two_cookie, "new-report").await.unwrap();
    assert_eq!(theirs.status(), 200);
    assert_eq!(
        theirs.json::<Value>().await.unwrap(),
        json!({ "deleted": 0 }),
        "another account's report is not this one's to delete"
    );
    let mine = delete_one(&user_one_cookie, "one%2Foff%20try")
        .await
        .unwrap();
    assert_eq!(mine.json::<Value>().await.unwrap(), json!({ "deleted": 1 }));
    let again = delete_one(&user_one_cookie, "one%2Foff%20try")
        .await
        .unwrap();
    assert_eq!(again.status(), 200);
    assert_eq!(
        again.json::<Value>().await.unwrap(),
        json!({ "deleted": 0 })
    );

    let deleted = client
        .delete(format!("{base}/api/reports"))
        .header("cookie", &user_one_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 200);
    assert_eq!(
        deleted.json::<Value>().await.unwrap(),
        json!({ "deleted": 2 })
    );

    let user_one_reports = client
        .get(format!("{base}/api/reports"))
        .header("cookie", &user_one_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(user_one_reports["reports"].as_array().unwrap().is_empty());

    let user_two_reports = client
        .get(format!("{base}/api/reports"))
        .header("cookie", &user_two_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let ids = user_two_reports["reports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|report| report["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["report-two"]);

    let logout = client
        .post(format!("{base}/api/logout"))
        .header("cookie", &user_one_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 200);
    let signed_out = client
        .get(format!("{base}/api/session"))
        .header("cookie", &user_one_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        signed_out,
        json!({"signedIn": false, "loginRequired": true, "deviceLogin": false, "maxDurationMin": MAX_DURATION_MIN}),
        "accounts exist here, so the browser has to know to demand a sign-in"
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// `/api/reports` is an authenticated write with no rate limit, and the client
/// picks its own row ids, so without a ceiling one account can grow the
/// database until the disk runs out. 507 rather than 429: waiting does not
/// help, so telling the client to retry would be a lie.
#[tokio::test]
async fn a_full_account_cannot_grow_the_report_database() {
    let (config, cookie, path) = signed_in_web_config("report-quota");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        for index in 0..MAX_REPORTS_PER_USER {
            connection.execute(
                "INSERT INTO reports (id, user_id, problem_id, payload, created_at, updated_at) VALUES (?1, 1, 'two-sum', '{}', 1, 1)",
                [format!("seeded-{index}")],
            ).unwrap();
        }
    }
    let (base, server) = spawn_web_server(config).await;
    let client = http_client();

    let refused = client
        .post(format!("{base}/api/reports"))
        .header("cookie", &cookie)
        .json(&json!({"id":"one-too-many","problemId":"three-sum","score":9}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 507);

    // The interview in progress still has to be able to save, or the ceiling
    // breaks the feature it protects.
    let rewritten = client
        .post(format!("{base}/api/reports"))
        .header("cookie", &cookie)
        .json(&json!({"id":"seeded-0","problemId":"two-sum","score":10}))
        .send()
        .await
        .unwrap();
    assert_eq!(rewritten.status(), 200);

    let stored = client
        .get(format!("{base}/api/reports"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        stored["reports"].as_array().unwrap().len() as i64,
        MAX_REPORTS_PER_USER,
        "the refused write must not have landed",
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// A report gets its own ceiling, and the endpoint has to use it.
///
/// `body_limit_matches_non_working_reference` pins the two constants and no
/// more: swapping `MAX_REPORT_BYTES` for `MAX_BODY_BYTES` at the one place the
/// report body is read passed every test in this file, and a graded report is
/// routinely larger than a session request. The failure would arrive as a 413
/// at the end of a finished interview, which is the worst moment to lose one.
///
/// Sized off the constants rather than off numbers, so raising either ceiling
/// keeps this asking the question it means.
#[tokio::test]
async fn a_report_is_bounded_by_its_own_ceiling_rather_than_the_session_one() {
    let (config, cookie, path) = signed_in_web_config("report-size");
    let (base, server) = spawn_web_server(config).await;
    let client = http_client();
    let url = format!("{base}/api/reports");
    let report = |id: &str, bytes: usize| {
        let summary = "s".repeat(bytes);
        json!({ "id": id, "problemId": "two-sum", "summary": summary })
    };

    // Past what a session request may be, and well inside what a report may be.
    let accepted = client
        .post(&url)
        .header("cookie", &cookie)
        .json(&report("roomy", MAX_BODY_BYTES * 4))
        .send()
        .await
        .unwrap();
    assert_eq!(
        accepted.status(),
        200,
        "a report larger than a session request is still a report"
    );

    let refused = client
        .post(&url)
        .header("cookie", &cookie)
        .json(&report("enormous", MAX_REPORT_BYTES))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 413, "and the ceiling is still a ceiling");

    // A body that is not JSON, and one naming no problem, are the client's
    // mistake and each says which.
    let malformed = client
        .post(&url)
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 400);
    assert_eq!(
        malformed.json::<Value>().await.unwrap()["error"],
        "Report must be JSON."
    );

    let unattributed = client
        .post(&url)
        .header("cookie", &cookie)
        .json(&json!({ "id": "orphan", "score": 7 }))
        .send()
        .await
        .unwrap();
    assert_eq!(unattributed.status(), 400);
    assert_eq!(
        unattributed.json::<Value>().await.unwrap()["error"],
        "Report is missing problemId."
    );

    // Only the accepted one landed: a refusal that had already written the row
    // would answer the same way and leave the account a report it never saved.
    let stored = client
        .get(&url)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        stored["reports"]
            .as_array()
            .unwrap()
            .iter()
            .map(|report| report["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["roomy"]
    );

    server.shutdown().await;
    remove_database(path).await;
}

#[test]
fn body_limit_matches_non_working_reference() {
    assert_eq!(MAX_BODY_BYTES, 8192);

    // Reports carry graded prose, so they get their own, larger ceiling; the
    // browser's per-field caps in web/lib.js are sized against this one.
    assert_eq!(MAX_REPORT_BYTES, 65536);
    assert_eq!(DEFAULT_WEB_DIR, "web");
}

/// An address nobody checked is not a delivery target, and a typed handle can
/// never acquire one.
#[tokio::test]
async fn self_declared_handle_cannot_authorize_delivery() {
    let db_path = account_db_path("self-declared-delivery");
    initialize_account_database(&db_path).unwrap();
    let mut config = web_config();
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(db_path.clone());
    config.recording = Some(recording_config());
    let (base, server) = spawn_web_server(config).await;
    let client = http_client();

    let cookie = record_login(&client, &base, "octocat").await;
    let refused = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert_eq!(
        refused.json::<Value>().await.unwrap()["code"],
        "recording_requires_verified_identity"
    );

    // Typing the same handle again mints another account rather than adopting
    // the first, so nobody can accumulate their way into a verified one. The
    // row count below is the assertion; `record_login` already fails if the
    // login itself did not succeed.
    record_login(&client, &base, "octocat").await;

    let connection = rusqlite::Connection::open(&db_path).unwrap();
    let mut statement = connection
        .prepare("SELECT github_id, email, email_verified FROM users ORDER BY github_id")
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2, "two logins, two accounts, never an upgrade");
    for (github_id, email, verified) in rows {
        assert!(github_id < 0, "a self-declared account keeps a negative id");
        assert_eq!(email, None, "and no address");
        assert_eq!(verified, 0, "and nothing claiming one was checked");
    }

    drop(statement);
    drop(connection);
    server.shutdown().await;
    remove_database(db_path).await;
}

/// An OAuth sign-in stores the primary verified address, and only that one.
#[tokio::test]
async fn github_callback_stores_only_the_primary_verified_email() {
    let (github_base, github_server) = spawn_mock_github().await;
    let path = account_db_path("verified-email");
    initialize_account_database(&path).unwrap();
    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    config.github_oauth_base_url = Some(github_base.clone());
    config.github_api_base_url = Some(github_base);
    config.recording = Some(recording_config());
    let (base, server) = spawn_web_server(config).await;

    let client = http_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let response = client
        .get(format!("{base}/api/callback?code=ok&state=state-token"))
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
    assert_eq!(response.status(), 302);
    let session_cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|cookie| cookie.starts_with("codetrial_session="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let (email, verified): (Option<String>, i64) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT email, email_verified FROM users WHERE github_id = 303",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        email.as_deref(),
        Some("octocat@example.test"),
        "the primary verified address, not the first entry and not the secondary one"
    );
    assert_eq!(verified, 1);

    // And that account can now start a recorded interview.
    let client = http_client();
    let interview = start_interview(&client, &base, &session_cookie).await;
    let allowed = client
        .post(format!("{base}/api/token"))
        .header("cookie", session_cookie)
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);

    server.shutdown().await;
    github_server.shutdown().await;
    remove_database(path).await;
}

/// The scope has to name what the callback then asks for. A widened fetch
/// against an unwidened authorize URL fails at GitHub, not here, and the
/// symptom is an address that is simply absent.
#[tokio::test]
async fn login_requests_the_email_scope_it_later_reads() {
    let db_path = account_db_path("email-scope");
    initialize_account_database(&db_path).unwrap();
    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(db_path.clone());
    let without_recording = config.clone();
    config.recording = Some(recording_config());
    let (base, server) = spawn_web_server(config).await;

    let authorize_url = |base: String| async move {
        let response = http_client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
            .get(format!("{base}/api/login"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 302);
        response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    };
    let location = authorize_url(base).await;

    // Decoded, not matched as an escaped literal. Percent-encoding has more
    // than one legal answer for a colon, and the thing that matters is the
    // scope GitHub will read.
    assert_eq!(
        query_parameter(&location, "scope").as_deref(),
        Some("read:user user:email"),
        "the authorize URL has to request the scope the callback then reads: {location}"
    );
    server.shutdown().await;

    // And a deployment that records nothing does not put a candidate's private
    // address on the consent screen, because it would never read it.
    let (base, server) = spawn_web_server(without_recording).await;
    assert_eq!(
        query_parameter(&authorize_url(base).await, "scope").as_deref(),
        Some("read:user")
    );

    server.shutdown().await;
    remove_database(db_path).await;
}

/// A recording block for tests that only care that recording is on.
/// The lobby offers lengths this deployment may not be able to record, and only
/// the server knows the cap. Without it in the payload the browser goes on
/// offering sixty minutes and `/api/token` shortens the interview after the
/// candidate has already asked for it, which is the substitution #16 is about.
#[tokio::test]
async fn the_session_reports_how_long_a_recorded_interview_may_run() {
    let mut config = web_config();
    config.recording = Some(recording_config());
    let (base, server) = spawn_web_server(config).await;

    let session = http_client()
        .get(format!("{base}/api/session"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();

    assert_eq!(session["maxDurationMin"], recording_config().max_minutes);
    server.shutdown().await;
}

/// A verified address is not given up because somebody else's server had a bad
/// minute.
///
/// The callback upserts whatever it was handed, so folding a rate limit into
/// "this account has no verified address" would take a candidate's delivery
/// target away permanently, on a transient failure they cannot see or retry.
#[tokio::test]
async fn a_failed_email_lookup_does_not_unverify_an_account() {
    let path = account_db_path("email-lookup-failure");
    initialize_account_database(&path).unwrap();

    // One good sign-in first, so there is something to lose.
    let (good_github, good_server) = spawn_mock_github().await;
    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    config.github_oauth_base_url = Some(good_github.clone());
    config.github_api_base_url = Some(good_github);
    config.recording = Some(recording_config());
    let (base, server) = spawn_web_server(config.clone()).await;
    let client = http_client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let callback = |base: String| {
        let client = client.clone();
        async move {
            client
                .get(format!("{base}/api/callback?code=ok&state=state-token"))
                .header(
                    "cookie",
                    format!(
                        "codetrial_oauth_state={}",
                        signed_cookie("state-token", "session-secret")
                    ),
                )
                .send()
                .await
                .unwrap()
        }
    };
    assert_eq!(callback(base.clone()).await.status(), 302);
    server.shutdown().await;
    good_server.shutdown().await;

    // Now the same account signs in again while /user/emails is rate limited.
    let (rate_limited, rate_limited_server) = spawn_rate_limited_github().await;
    let mut second = config;
    second.github_oauth_base_url = Some(rate_limited.clone());
    second.github_api_base_url = Some(rate_limited);
    let (base, server) = spawn_web_server(second).await;
    let failed = callback(base).await;
    assert_eq!(
        failed.status(),
        502,
        "a failed lookup fails the sign-in rather than downgrading the account"
    );

    // Both 502 legs of the callback carry the same status, so the status alone
    // would still pass here if the token exchange had broken and the profile
    // request were never reached. The message is what says this run got as far
    // as the leg the test is about.
    let failed = failed.json::<Value>().await.unwrap();
    assert_eq!(
        failed["error"], "GitHub profile request failed.",
        "the rate limit fails the profile request, not the token exchange"
    );

    let (email, verified): (Option<String>, i64) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT email, email_verified FROM users WHERE github_id = 303",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(email.as_deref(), Some("octocat@example.test"));
    assert_eq!(verified, 1, "the address survives the outage");

    server.shutdown().await;
    rate_limited_server.shutdown().await;
    remove_database(path).await;
}

/// Every route that acts on an account rejects a request carrying no session.
///
/// These handlers take an `Owner` extractor rather than checking inline. The
/// compiler proves the extractor runs; this proves what it answers when nobody
/// is signed in, for every owner-scoped route rather than the five that had
/// coverage scattered through this file.
///
/// The second half keeps the first half honest. The table is hand-written, so
/// a thirteenth owner-scoped route would otherwise join nothing: instead every
/// route `declared_routes` finds has to appear either here or in the anonymous
/// list, and a new one fails until someone says which it is.
#[tokio::test]
async fn every_owner_scoped_route_refuses_an_anonymous_request() {
    let (base, server, path, client, _cookie) = recorded_server("anon-owner-routes").await;
    let owner_scoped = [
        // Owner-scoped despite not being an `Owner` extractor: it resolves the
        // session itself and then asks whether that account is the one
        // `/api/token` minted the room for. It sat in the anonymous list below
        // for want of anywhere else, which said the opposite.
        (reqwest::Method::POST, "/api/observer-token"),
        (reqwest::Method::GET, "/api/task-reviews/{room}"),
        (
            reqwest::Method::GET,
            "/api/task-sets/{set_id}/tasks/{task_id}",
        ),
        (reqwest::Method::POST, "/api/task-sets/{set_id}/unlock"),
        (
            reqwest::Method::POST,
            "/api/task-sets/{set_id}/tasks/{task_id}/attempts",
        ),
        (reqwest::Method::GET, "/api/reports"),
        (reqwest::Method::POST, "/api/reports"),
        (reqwest::Method::DELETE, "/api/reports"),
        (reqwest::Method::DELETE, "/api/reports/{id}"),
        (reqwest::Method::POST, "/api/interviews"),
        (reqwest::Method::DELETE, "/api/interviews/{id}/consent"),
        (reqwest::Method::POST, "/api/interviews/{id}/recording"),
        (reqwest::Method::GET, "/api/interviews/{id}/recording"),
        (reqwest::Method::POST, "/api/interviews/{id}/end"),
        (reqwest::Method::POST, "/api/interviews/{id}/events"),
        (reqwest::Method::GET, "/api/interviews/{id}/snapshot"),
        (reqwest::Method::GET, "/api/recordings"),
        (reqwest::Method::GET, "/api/recordings/{id}"),
        (reqwest::Method::GET, "/api/recordings/{id}/events"),
    ];

    for (method, route) in owner_scoped.clone() {
        let target = route.replace("{id}", "some-interview");
        let status = client
            .request(method.clone(), format!("{base}{target}"))
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, 401, "{method} {target} answered {status}, not 401");
    }

    // Routes that answer without a session. The two LiveKit paths belong here
    // because they authenticate a signed machine principal against a room
    // rather than a person against a cookie.
    let anonymous = [
        "/healthz",
        "/api/token",
        "/api/login",
        "/api/callback",
        // A device sign-in creates the session these routes would ask for.
        "/api/github/device",
        "/api/github/device/poll",
        "/api/session",
        "/api/logout",
        "/runtime-config.js",
        codetrial::recording::WEBHOOK_ROUTE,
        codetrial::recording::REPLAY_ROUTE,
    ];
    for route in declared_routes() {
        let known = owner_scoped.iter().any(|(_, path)| *path == route)
            || anonymous.contains(&route.as_str());
        assert!(
            known,
            "{route} is in the router but is classified neither owner-scoped nor \
             anonymous; add it to one of the two lists in this test"
        );
    }

    server.shutdown().await;
    remove_database(path).await;
}

/// `Provider` and `RecordingConfig` both hand-write a redacting `Debug`, and
/// both name `WebServerConfig` as the reason: it derives `Debug`, so anything
/// reachable from it can reach a log line. The two secrets the struct holds
/// directly were the ones nobody checked, and the derive printed both in full.
///
/// `session_secret` is why this is a test rather than a style note. It signs
/// the session cookie, so a copy on stderr is not a credential to steal but the
/// ability to mint any user's session.
#[test]
fn web_server_config_debug_redacts_its_own_secrets() {
    let mut config = web_config();
    config.github_client_id = Some("public-client-id".to_string());
    config.github_client_secret = Some("CLIENTSECRET-def456".to_string());
    config.session_secret = Some("SESSIONSECRET-abc123".to_string());

    let printed = format!("{config:?}");
    for secret in ["CLIENTSECRET-def456", "SESSIONSECRET-abc123"] {
        assert!(
            !printed.contains(secret),
            "Debug output leaked {secret}: {printed}"
        );
    }
    assert!(
        printed.contains("<redacted>"),
        "the redaction should be visible rather than silent: {printed}"
    );

    // The pool redacts one level down, which is what makes the whole struct
    // safe to print rather than just its top level.
    assert!(
        !printed.contains("devsecret"),
        "the provider secret leaked through the pool: {printed}"
    );

    // Absent stays absent: `None` must not print as a redacted value, or an
    // operator reading a log cannot tell a configured secret from a missing
    // one.
    let blank = format!("{:?}", web_config());
    assert!(
        blank.contains("session_secret: None"),
        "an unset secret should read as None: {blank}"
    );

    // The client id is not a secret, and redacting it would cost the one field
    // that makes an OAuth misconfiguration diagnosable from a log.
    assert!(
        printed.contains("public-client-id"),
        "the client id is public and should stay readable: {printed}"
    );
}

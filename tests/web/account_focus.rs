use super::*;

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
    let mut owners = Vec::new();
    for cookie in [&first, &first, &second] {
        let session = client
            .get(format!("{base}/api/session"))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(session["user"]["login"], "octocat");
        owners.push(session["user"]["id"].as_str().unwrap().to_owned());
    }
    assert_eq!(owners[0], owners[1], "an account has a stable owner id");
    assert_ne!(owners[0], owners[2], "typed handles do not identify owners");

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

#[tokio::test]
async fn token_api_matches_frontend_contract_over_http() {
    let (config, cookie, db_path) = signed_in_web_config("contract");
    let (base, server) = spawn_web_server(config).await;
    let response = http_client()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .json(&json!({"problemId":"merge-intervals","durationMin":120}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let mut keys = body.as_object().unwrap().keys().collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "accountId",
            "durationMin",
            "guest",
            "roomName",
            "serverUrl",
            "token"
        ]
    );
    let session: Value = http_client()
        .get(format!("{base}/api/session"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(body["accountId"].is_string());
    assert_eq!(body["accountId"], session["user"]["id"]);
    let reports: Value = http_client()
        .get(format!("{base}/api/reports"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        reports["accountId"], session["user"]["id"],
        "session and reports must expose the same account owner"
    );
    assert_eq!(
        body["accountId"], reports["accountId"],
        "token and reports must expose the same account owner"
    );
    assert_eq!(body["guest"], false);
    assert_eq!(body["serverUrl"], "wss://example.livekit.cloud");
    assert!(body["roomName"].as_str().unwrap().starts_with("interview-"));

    let claims = claims(body["token"].as_str().unwrap());
    assert_eq!(claims["name"], "Candidate");
    assert!(claims["sub"].as_str().unwrap().starts_with("candidate-"));
    assert_eq!(
        claims["exp"].as_u64().unwrap() - claims["nbf"].as_u64().unwrap(),
        TOKEN_TTL_SECONDS
    );
    assert_eq!(claims["video"]["room"], body["roomName"]);
    assert_eq!(claims["video"]["roomJoin"], true);
    assert_eq!(claims["video"]["canPublish"], true);
    assert_eq!(claims["video"]["canSubscribe"], true);
    assert_eq!(claims["video"]["canPublishData"], true);
    let metadata: Value = serde_json::from_str(claims["metadata"].as_str().unwrap()).unwrap();
    assert_eq!(metadata["problemId"], "merge-intervals");
    assert_eq!(metadata["durationMin"], 90);
    assert_eq!(metadata["candidateIdentity"], claims["sub"]);

    // The page cannot read the metadata: it is inside a signed token it never
    // opens. The response body carries the length the agent will enforce, which
    // for a whole-minute request is the metadata value itself.
    assert_eq!(body["durationMin"], metadata["durationMin"]);

    server.shutdown().await;
    remove_database(db_path).await;
}

#[tokio::test]
async fn guest_token_admission_has_no_account_owner() {
    let (mut config, _, db_path) = signed_in_web_config("guest-focus-owner");
    config.guest_mode_enabled = true;
    let (base, server) = spawn_web_server_with_guest_dispatcher(config).await;
    let response = http_client()
        .post(format!("{base}/api/token"))
        .json(&json!({"problemId": "two-sum", "durationMin": 45}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["guest"], true);
    assert!(body.get("accountId").is_none());

    server.shutdown().await;
    remove_database(db_path).await;
}

#[tokio::test]
async fn recycling_a_throwaway_user_row_does_not_recycle_the_browser_owner() {
    let (base, server, path, client) = account_server("recycled-focus-owner").await;
    let first_cookie = record_login(&client, &base, "first-owner").await;
    let first_session = client
        .get(format!("{base}/api/session"))
        .header("cookie", &first_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let first_owner = first_session["user"]["id"].as_str().unwrap().to_owned();
    let connection = rusqlite::Connection::open(&path).unwrap();
    let (first_user_id, first_account_identity): (i64, i64) = connection
        .query_row(
            "SELECT id, github_id FROM users WHERE login = 'first-owner'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    connection
        .execute(
            "UPDATE sessions SET expires_at = 0 WHERE user_id = ?1",
            [first_user_id],
        )
        .unwrap();
    drop(connection);
    let accounts = codetrial::accounts::Accounts::open(codetrial::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "session-secret".to_string(),
        db_path: path.clone(),
        oauth_base_url: "https://github.com".to_string(),
        api_base_url: "https://api.github.com".to_string(),
    })
    .unwrap();
    assert_eq!(
        codetrial::accounts::sweep_expired_sessions(&accounts, 1).unwrap(),
        (1, 1),
    );
    drop(accounts);
    let second_cookie = record_login(&client, &base, "second-owner").await;
    let second_session = client
        .get(format!("{base}/api/session"))
        .header("cookie", &second_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let second_owner = second_session["user"]["id"].as_str().unwrap().to_owned();
    let connection = rusqlite::Connection::open(&path).unwrap();
    let (second_user_id, second_account_identity): (i64, i64) = connection
        .query_row(
            "SELECT id, github_id FROM users WHERE login = 'second-owner'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();

    assert_eq!(
        first_user_id, second_user_id,
        "SQLite should reproduce the recycled users.id case",
    );
    assert_ne!(
        first_account_identity, second_account_identity,
        "throwaway accounts need distinct stable identities",
    );
    assert_ne!(
        first_owner, second_owner,
        "recycling users.id must not recycle the browser owner",
    );
    drop(connection);
    server.shutdown().await;
    remove_database(path).await;
}

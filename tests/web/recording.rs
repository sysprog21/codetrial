//! Consent, egress, the webhook that moves a row, and reading a replay back.
//!
//! Split out of `tests/web.rs`, which is still the test target: cargo
//! discovers only `tests/*.rs`, so this compiles as a module of that one
//! binary rather than relinking the crate for a file of its own.

use super::*;

#[test]
fn account_recording_config_does_not_require_github_oauth() {
    let mut config = web_config();

    assert!(login_config(&config).is_none());

    config.session_secret = Some("session".to_string());
    config.db_path = Some(Path::new("codetrial.db").to_path_buf());

    let login = login_config(&config).expect("recording config should enable account sessions");

    // `is_none` rather than `assert_eq!(.., None)`: that form needs `Debug` on
    // `GitHubOauth`, and the client secret is the reason that type does not
    // have one.
    assert!(
        login.oauth.is_none(),
        "no OAuth app means no credentials, not blank ones"
    );
    assert_eq!(login.session_secret, "session");
    assert_eq!(login.db_path, Path::new("codetrial.db"));
}

/// Consent exists before anything could be recorded, and `/api/token` is where
/// that is enforced.
///
/// The token is what precedes an Egress call, so a token minted without a
/// persisted consent row is the one ordering this feature cannot survive. The
/// enforcement is the refusal; a comment saying "call this first" is not.
#[tokio::test]
async fn interviews_persist_consent_before_egress() {
    let (base, server, path, client, cookie) = recorded_server("consent-before-egress").await;

    // No interview id: refused, and nothing is written.
    let refused = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert_eq!(
        refused.json::<Value>().await.unwrap()["code"],
        "recording_requires_consent"
    );
    assert_eq!(interview_rows(&path).len(), 0);

    // A stale page that shows older wording is refused rather than recorded as
    // having agreed to text it never displayed.
    let stale = client
        .post(format!("{base}/api/interviews"))
        .header("cookie", cookie.clone())
        .json(&json!({ "consentVersion": "1970-01-01" }))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 409);
    assert_eq!(
        stale.json::<Value>().await.unwrap()["code"],
        "consent_version_mismatch"
    );
    assert_eq!(interview_rows(&path).len(), 0);

    let interview = start_interview(&client, &base, &cookie).await;
    let rows = interview_rows(&path);
    assert_eq!(
        rows.len(),
        1,
        "consent is one row, written before the token"
    );
    let (id, version, consent_at, withdrawn) = rows[0].clone();
    assert_eq!(id, interview);
    assert_eq!(version, codetrial::recording::CONSENT_VERSION);
    assert!(consent_at > 0, "consent is a fact with a time");
    assert_eq!(withdrawn, None);

    // A reload is not a second consent. Same person, same wording, same
    // interview that has not started, so the pending row is handed back rather
    // than spending another of the account's rows for nothing.
    assert_eq!(
        start_interview(&client, &base, &cookie).await,
        interview,
        "agreeing again before starting returns the pending interview"
    );
    assert_eq!(interview_rows(&path).len(), 1);

    let allowed = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);

    // One consent is one interview. Without the claim, the same id mints
    // recorded rooms without limit, and a candidate who agreed to be recorded
    // once would have agreed to all of them.
    let reused = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(reused.status(), 403);
    assert_eq!(
        reused.json::<Value>().await.unwrap()["code"],
        "recording_requires_consent"
    );

    // The room the consent authorized is written down, so a later step can find
    // the consent a room was started under.
    let room: Option<String> = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT room_name FROM interviews WHERE id = ?1",
            [&interview],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        room.is_some_and(|room| room.starts_with("interview-")),
        "the claim records which room it was spent on"
    );

    // Somebody else's interview id is not consent. It answers the same way a
    // missing one does, because telling them apart enumerates other people's
    // interviews.
    let (other_cookie, _) = second_account(&client, &base, &path).await;
    let other = start_interview(&client, &base, &other_cookie).await;
    let borrowed = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie)
        .json(&json!({ "interviewId": other }))
        .send()
        .await
        .unwrap();
    assert_eq!(borrowed.status(), 403);

    server.shutdown().await;
    remove_database(path).await;
}

/// Withdrawal is recorded, and a withdrawn interview cannot start another
/// recorded room.
///
/// The transition of an active recording to `failed` belongs to the recording
/// lifecycle. What this route owns is the state that transition reads, and the
/// refusal that stops the candidate who just said no from reloading into a
/// second recorded interview.
#[tokio::test]
async fn consent_withdrawal_stops_egress() {
    let provider = std::sync::Arc::new(FakeRecordingProvider::default());
    let (base, server, path, client, cookie) =
        recorded_server_with_provider("consent-withdrawal", provider.clone()).await;
    let interview = start_interview(&client, &base, &cookie).await;

    // A real recording, started through the route a candidate reaches, so the
    // withdrawal below has something to stop rather than an empty interview.
    let token = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(token.status(), 200);
    let started = client
        .post(format!("{base}/api/interviews/{interview}/recording"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), 202, "the provider has been asked");
    assert_eq!(
        started.json::<Value>().await.unwrap()["state"],
        "recording",
        "the row moved when the egress id landed, and the answer says so"
    );
    assert_eq!(provider.starts(), 1);

    let withdrawn = client
        .delete(format!("{base}/api/interviews/{interview}/consent"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(withdrawn.status(), 204);

    // The recording is abandoned, not finished: `failed` with the reason a
    // later deletion is justified by, and the provider told to stop.
    assert_eq!(provider.stops().len(), 1, "the Egress job is stopped");
    let (state, error): (String, Option<String>) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT state, error FROM recordings WHERE interview_id = ?1",
            [&interview],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "failed");
    assert_eq!(error.as_deref(), Some("consent_withdrawn"));

    let rows = interview_rows(&path);
    assert_eq!(rows.len(), 1, "withdrawal records, it does not delete");
    let first_withdrawal = rows[0].3.expect("the withdrawal has a time");

    // Idempotent, and the first time is the one that counts.
    let again = client
        .delete(format!("{base}/api/interviews/{interview}/consent"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 204);
    assert_eq!(interview_rows(&path)[0].3, Some(first_withdrawal));

    let refused = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert_eq!(
        refused.json::<Value>().await.unwrap()["code"],
        "recording_requires_consent"
    );

    // Nobody else can withdraw it, and the refusal does not confirm it exists.
    let (other_cookie, _) = second_account(&client, &base, &path).await;
    let mine = start_interview(&client, &base, &cookie).await;
    let stranger = client
        .delete(format!("{base}/api/interviews/{mine}/consent"))
        .header("cookie", other_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(stranger.status(), 404);
    assert_eq!(
        interview_rows(&path)
            .iter()
            .find(|(id, ..)| *id == mine)
            .expect("the interview is still there")
            .3,
        None,
        "a stranger's request changes nothing"
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// A second start does not make a second Egress job, and is not told the
/// recording has not begun.
///
/// The row moves underneath the handler: `starting` becomes `recording` when
/// the first caller's egress id lands. A second caller that answered from the
/// snapshot it read would tell the browser to keep waiting for a recording that
/// was already running.
#[tokio::test]
async fn a_second_start_reports_the_state_the_row_holds() {
    let provider = std::sync::Arc::new(FakeRecordingProvider::default());
    let (base, server, path, client, cookie) =
        recorded_server_with_provider("second-start", provider.clone()).await;
    let interview = start_interview(&client, &base, &cookie).await;
    assert_eq!(
        client
            .post(format!("{base}/api/token"))
            .header("cookie", cookie.clone())
            .json(&json!({ "interviewId": interview }))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    let start = || {
        let client = client.clone();
        let base = base.clone();
        let cookie = cookie.clone();
        let interview = interview.clone();
        async move {
            client
                .post(format!("{base}/api/interviews/{interview}/recording"))
                .header("cookie", cookie)
                .send()
                .await
                .unwrap()
        }
    };

    let first = start().await;
    assert_eq!(first.status(), 202);
    assert_eq!(first.json::<Value>().await.unwrap()["state"], "recording");

    let second = start().await;
    assert_eq!(second.status(), 202);
    assert_eq!(
        second.json::<Value>().await.unwrap()["state"],
        "recording",
        "the second caller reads the row, not the snapshot it started from"
    );
    assert_eq!(provider.starts(), 1, "one interview, one Egress job");

    server.shutdown().await;
    remove_database(path).await;
}

/// The row moves while the provider is answering, and the answer says so.
///
/// This is the interleaving the snapshot hid: the handler reads a `starting`
/// row, a sweeper retry stores its own egress id, and the handler comes back
/// with an id the row will not take. It has to stop the job it made and report
/// the state that is actually there.
#[tokio::test]
async fn a_start_overtaken_mid_flight_stops_its_own_job() {
    let provider = std::sync::Arc::new(FakeRecordingProvider::default());
    let (base, server, path, client, cookie) =
        recorded_server_with_provider("overtaken", provider.clone()).await;
    let interview = start_interview(&client, &base, &cookie).await;
    assert_eq!(
        client
            .post(format!("{base}/api/token"))
            .header("cookie", cookie.clone())
            .json(&json!({ "interviewId": interview }))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    *provider.preempt.lock().unwrap() = Some(path.clone());

    let response = client
        .post(format!("{base}/api/interviews/{interview}/recording"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    assert_eq!(
        response.json::<Value>().await.unwrap()["state"],
        "recording",
        "the state comes from the row, which somebody else moved"
    );

    // The job this call made is not the job the row names, so it stops its own
    // rather than leaving two running.
    assert_eq!(provider.stops(), vec!["EG_web_fake".to_string()]);
    let egress: Option<String> = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT egress_id FROM recordings WHERE interview_id = ?1",
            [&interview],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        egress.as_deref(),
        Some("EG_swept"),
        "the id that got there first is the one that stays"
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// A completion carrying nothing is a failure, driven through the real webhook.
///
/// The predicate is unit tested; this is the path a candidate's recording
/// actually takes, signature and all. Sending an empty completion through the
/// transfer shared a zero-byte video.
#[tokio::test]
async fn a_completion_with_no_file_fails_the_recording() {
    let provider = std::sync::Arc::new(FakeRecordingProvider::default());
    let (base, server, path, client, cookie) =
        recorded_server_with_provider("empty-completion", provider.clone()).await;
    let interview = start_interview(&client, &base, &cookie).await;
    assert_eq!(
        client
            .post(format!("{base}/api/token"))
            .header("cookie", cookie.clone())
            .json(&json!({ "interviewId": interview }))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .post(format!("{base}/api/interviews/{interview}/recording"))
            .header("cookie", cookie.clone())
            .send()
            .await
            .unwrap()
            .status(),
        202
    );

    let ended = json!({
        "event": "egress_ended",
        "id": "EV_empty",
        "createdAt": "1770000123",
        "egressInfo": {
            "egressId": "EG_web_fake",
            "status": "EGRESS_COMPLETE",
            "fileResults": []
        }
    })
    .to_string();
    let response = post_webhook(&client, &base, &ended).await;
    assert_eq!(response.status(), 200);

    let (state, error): (String, Option<String>) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT state, error FROM recordings WHERE interview_id = ?1",
            [&interview],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "failed");
    assert_eq!(error.as_deref(), Some("partial_output"));

    // And the status route says what to do about it, which "failed" does not.
    let status = client
        .get(format!("{base}/api/interviews/{interview}/recording"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), 200);
    let body = status.json::<Value>().await.unwrap();
    assert_eq!(body["error"], "partial_output");
    assert_eq!(body["recovery"], "start_again");
    for handle in ["gcsObject", "driveFileId", "drivePermissionId"] {
        assert!(
            body.get(handle).is_none(),
            "the status route must not hand back {handle}"
        );
    }

    server.shutdown().await;
    remove_database(path).await;
}

/// A malformed body is a client error, whether or not this server records.
///
/// The consent check reads the body too, and it answers 403 to anything it
/// cannot parse. Letting that answer win sends a candidate looking for a
/// checkbox they already ticked.
#[tokio::test]
async fn a_malformed_body_is_a_bad_request_even_on_a_recording_server() {
    let (base, server, path, client, cookie) = recorded_server("malformed-recorded").await;
    start_interview(&client, &base, &cookie).await;

    let response = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie)
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"],
        "Session request must be JSON."
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// A request that fails after the consent check must not spend the consent.
///
/// The claim used to run before the token was signed and before an interviewer
/// was dispatched, so a busy server burned the candidate's only consent and
/// then told them to retry, and the retry was refused.
#[tokio::test]
async fn a_refused_interview_leaves_its_consent_unspent() {
    let (mut config, cookie, path) = signed_in_web_config("consent-unspent");
    config.recording = Some(recording_config());
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE users SET email = 'one@example.test', email_verified = 1 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    let dispatcher = std::sync::Arc::new(RecordingDispatcher::default());
    dispatcher
        .at_capacity
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let (base, server) = spawn_web_server_with_dispatcher(config, dispatcher.clone()).await;
    let client = reqwest::Client::new();

    let interview = start_interview(&client, &base, &cookie).await;
    let busy = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(busy.status(), 503, "the server is full, not the candidate");

    let room: Option<String> = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT room_name FROM interviews WHERE id = ?1",
            [&interview],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(room, None, "a refused request spends nothing");

    // And the retry the candidate was told to make works.
    dispatcher
        .at_capacity
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let retried = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie)
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(retried.status(), 200);

    server.shutdown().await;
    remove_database(path).await;
}

/// The per-batch caps bound one request and the stored quota bounds what an
/// interview keeps. Neither bounds how often one account may ask, and an ask
/// that is not refused before the body is a parse and a stored batch. This is
/// the refusal.
#[tokio::test]
async fn replay_ingestion_rate_limits_a_looping_client() {
    let (base, server, path, client, cookie) = recorded_server("replay-rate-limit").await;
    let interview = start_interview(&client, &base, &cookie).await;
    let url = format!("{base}/api/interviews/{interview}/events");
    let batch = json!({
        "events": [{
            "v": codetrial::recording::REPLAY_VERSION,
            "kind": "transcript",
            "at": 1_770_000_000_000i64,
            "payload": { "text": "hello" }
        }]
    });

    // Sent together rather than one after another, because the limiter's window
    // is sixty seconds and this is a hundred and twenty round trips.
    // Sequential, the test quietly assumed all of them finish inside that
    // window: on a loaded machine they do not, the window rolls over mid-loop,
    // and the request below starts a fresh one and answers 200. That read as a
    // flaky rate-limit test and it was this.
    //
    // Order does not matter to what is being asserted. The limiter admits the
    // first `REPLAY_RATE_LIMIT` asks in a window and refuses the next, so every
    // one of these is allowed whichever way they interleave, and the refusal
    // below is the first ask past the limit however they landed.
    let mut inflight = Vec::new();
    for attempt in 1..=REPLAY_RATE_LIMIT {
        let (client, url, cookie, batch) =
            (client.clone(), url.clone(), cookie.clone(), batch.clone());
        inflight.push(tokio::spawn(async move {
            let response = client
                .post(&url)
                .header("cookie", &cookie)
                .json(&batch)
                .send()
                .await
                .unwrap();
            (attempt, response.status())
        }));
    }
    for handle in inflight {
        let (attempt, status) = handle.await.unwrap();
        assert_eq!(status, 200, "batch {attempt} should be allowed");
    }

    let blocked = client
        .post(&url)
        .header("cookie", &cookie)
        .json(&batch)
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 429);
    assert_eq!(blocked.headers().get("retry-after").unwrap(), "60");

    // Refused before the body, so the refused batch stored nothing. A limiter
    // that ran after the insert would answer 429 to a client whose events it
    // had already kept.
    let stored: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM replay_events WHERE interview_id = ?1",
            [&interview],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, i64::from(REPLAY_RATE_LIMIT));

    server.shutdown().await;
    remove_database(path).await;
}

/// Reading a recording costs budget too, and the three read routes share one.
///
/// Every one of them was already owner scoped and already bounded per answer,
/// so what was missing was a bound on how often, and the review read added for
/// the response window is what made that worth closing: it returns every
/// `avatar` and `lifecycle` transition where the snapshot returns the newest of
/// each.
///
/// Spent across the three routes rather than one, because they share a bucket
/// and a test that spent it on one route would pass on a server that gave each
/// route its own.
#[tokio::test]
async fn recording_reads_share_one_rate_limit() {
    let (base, server, path, client, cookie) = recorded_server("read-rate-limit").await;
    let interview = start_interview(&client, &base, &cookie).await;
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        ) VALUES ('rec-read', 1, ?1, 'interview-abc12345', ?1,
            'one@example.test', 'ready', 10, 10)
        ",
            [&interview],
        )
        .unwrap();

    let paths = [
        "/api/recordings".to_string(),
        "/api/recordings/rec-read".to_string(),
        "/api/recordings/rec-read/events".to_string(),
        "/api/recordings/rec-read/events?avatar=history".to_string(),
    ];
    for spent in 0..READ_RATE_LIMIT {
        let url = format!("{base}{}", paths[spent as usize % paths.len()]);
        let response = client
            .get(&url)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "read {spent} should be allowed");
    }

    // Every route, because one bucket means the next request is refused
    // whichever of them asks.
    for path_suffix in &paths {
        let blocked = client
            .get(format!("{base}{path_suffix}"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(blocked.status(), 429, "{path_suffix}");
        assert_eq!(blocked.headers().get("retry-after").unwrap(), "60");
    }

    // A different account is untouched, which is what keying on the account
    // rather than the address buys: both accounts reach this server on the same
    // loopback address, so an address-keyed bucket would refuse this read too.
    // It asks for its own listing, the one read of the three it owns anything
    // to answer.
    let (other_cookie, _) = second_account(&client, &base, &path).await;
    let other = client
        .get(format!("{base}/api/recordings"))
        .header("cookie", &other_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), 200);

    // Posting a replay still works too: the read budget and the write budget
    // are separate buckets, so a reviewer reading a history cannot lock a
    // candidate out of recording their own interview.
    let posted = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", &cookie)
        .json(&json!({ "events": [envelope("transcript", json!({ "text": "hello" }))] }))
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), 200);

    server.shutdown().await;
    remove_database(path).await;
}

/// The replay is the account's own, and only the account's.
///
/// The route is the one place an interview id arrives from a browser, so the
/// owner check is what stands between a replay and anyone who can guess an id.
#[tokio::test]
async fn replay_events_are_owner_scoped() {
    let (base, server, path, client, cookie) = recorded_server("replay-owner").await;
    let interview = start_interview(&client, &base, &cookie).await;

    let event = json!({
        "v": codetrial::recording::REPLAY_VERSION,
        "kind": "transcript",
        "at": 1_770_000_000_000i64,
        "payload": { "text": "hello" }
    });
    let stored = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [event, event] }))
        .send()
        .await
        .unwrap();
    assert_eq!(stored.status(), 200);
    let body = stored.json::<Value>().await.unwrap();
    assert_eq!(
        (body["firstSeq"].as_i64(), body["lastSeq"].as_i64()),
        (Some(0), Some(1))
    );

    // Somebody else's interview, by id. Not a 403: an account that does not own
    // an interview should not learn that it exists.
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "INSERT INTO users (id, github_id, login, email, email_verified, created_at, updated_at)
                   VALUES (2, 202, 'two', 'two@example.test', 1, 1, 1);
                 INSERT INTO interviews (id, account_id, consent_version, consent_at)
                   VALUES ('int-theirs', 2, '2026-08-21', 1);",
            )
            .unwrap();
    }
    let theirs = client
        .post(format!("{base}/api/interviews/int-theirs/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [event] }))
        .send()
        .await
        .unwrap();
    assert_eq!(theirs.status(), 404);
    let count: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM replay_events WHERE interview_id = 'int-theirs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);

    // And a batch nobody sent by hand.
    let flood: Vec<Value> = std::iter::repeat_n(event.clone(), 200).collect();
    let refused = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": flood }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 413);

    // A batch with nothing in it gets no range of sequence numbers, because
    // none were allocated.
    let empty = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(empty.status(), 400);
    assert_eq!(
        empty.json::<Value>().await.unwrap()["code"],
        "replay_envelope_invalid"
    );

    let signed_out = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .json(&json!({ "events": [event] }))
        .send()
        .await
        .unwrap();
    assert_eq!(signed_out.status(), 401);

    // The byte ceiling, over HTTP, with one row standing in for eight megabytes
    // of replay: what is under test is the refusal and the flag, not SQLite's
    // ability to hold five thousand rows.
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        ) VALUES ('rec-replay', 1, ?1, 'interview-abc12345', ?1,
            'one@example.test', 'recording', 1, 1)
        ",
                [&interview],
            )
            .unwrap();
        connection
            .execute(
                "
        INSERT INTO replay_events (interview_id, seq, kind, at, payload, bytes, received_at)
        VALUES (?1, 9999, 'transcript', 1, '{}', ?2, 1)
        ",
                rusqlite::params![&interview, codetrial::recording::MAX_REPLAY_BYTES],
            )
            .unwrap();
    }
    let over = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [event] }))
        .send()
        .await
        .unwrap();
    assert_eq!(over.status(), 413);
    assert_eq!(
        over.json::<Value>().await.unwrap()["code"],
        "replay_quota_exceeded"
    );
    let (state, flag): (String, i64) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT state, quota_exceeded FROM recordings WHERE id = 'rec-replay'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (state.as_str(), flag),
        ("recording", 1),
        "the replay stopped growing; the recording did not fail"
    );

    // Withdrawal closes ingest. A browser with a buffer will flush it after the
    // candidate has said stop, and a replay that keeps growing past a
    // withdrawal is not a withdrawal.
    {
        let withdrawn = client
            .delete(format!("{base}/api/interviews/{interview}/consent"))
            .header("cookie", cookie.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(withdrawn.status(), 204);
    }
    let flushed = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [event] }))
        .send()
        .await
        .unwrap();
    assert_eq!(flushed.status(), 404);
    let stored_after: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM replay_events WHERE interview_id = ?1",
            [&interview],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        stored_after, 3,
        "the two stored above plus the synthetic quota row, and nothing after the withdrawal"
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// The recorder reads the replay with the credential it was given.
///
/// The Egress browser holds no session cookie, so this is the one replay read
/// authorized by a room token. What matters is that the token is verified
/// rather than parsed, and that it only opens the room it names.
#[tokio::test]
async fn the_recorder_reads_the_replay_for_the_room_its_token_names() {
    let (base, server, path, client, cookie) = recorded_server("replay-room-token").await;
    let interview = start_interview(&client, &base, &cookie).await;
    let room = "interview-abc12345";
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        ) VALUES ('rec-room', 1, ?1, ?2, ?1, 'one@example.test', 'recording', 1, 1)
        ",
            [&interview, &room.to_string()],
        )
        .unwrap();
    let event = |kind: &str, text: &str| envelope(kind, json!({ "text": text }));
    let posted = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        // The `avatar` and `lifecycle` rows are what make the assertion below
        // able to fail. They are the only kinds the snapshot's collapse and the
        // review read's differ on, so without them the template's answer is the
        // same either way and "the template still gets the collapsed snapshot"
        // held whether or not this route honoured the parameter, which is the
        // one thing it exists to catch.
        .json(&json!({ "events": [
            envelope("avatar", json!({ "state": "speaking" })),
            event("editor", "first draft"),
            event("transcript", "hello"),
            envelope("avatar", json!({ "state": "listening" })),
            envelope("lifecycle", json!({ "state": "paused" })),
            event("editor", "second draft"),
        ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), 200);

    let token_for = |room: &str| {
        codetrial::token::livekit_token(codetrial::token::LivekitTokenInput {
            api_key: "devkey",
            api_secret: "devsecret",
            identity: "EG_recorder",
            name: "recorder",
            room,
            metadata: "",
            agent: false,
            now_seconds: codetrial::current_epoch_seconds(),
        })
        .unwrap()
    };
    let replay = |token: String, query: &str| {
        let url = format!("{base}/api/recording/replay{query}");
        let client = client.clone();
        async move {
            client
                .get(url)
                .header("authorization", token)
                .send()
                .await
                .unwrap()
        }
    };

    let snapshot = replay(token_for(room), "").await;
    assert_eq!(snapshot.status(), 200);
    let body = snapshot.json::<Value>().await.unwrap();
    assert_eq!(body["seq"], 5);
    assert_eq!(
        body["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["transcript", "avatar", "lifecycle", "editor"],
        "the snapshot drops the superseded editor and avatar frames"
    );

    // The template's read does not take the review page's parameter, and this
    // is the assertion that says so. The whole reason `Avatar` stays a snapshot
    // kind is that a template joining late should learn Jim's current state
    // without replaying the interview; a refactor that plumbed `avatar=history`
    // through to here would undo that with nothing to notice.
    let unwidened = replay(token_for(room), "?avatar=history").await;
    assert_eq!(unwidened.status(), 200);
    assert_eq!(
        unwidened.json::<Value>().await.unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["transcript", "avatar", "lifecycle", "editor"],
        "the template still gets the collapsed snapshot, parameter or no parameter"
    );

    // The tail keeps every frame, superseded or not: a reader carrying on from
    // a snapshot is replaying, and a frame it never saw is not one to skip.
    let tail = replay(token_for(room), "?after=0").await;
    assert_eq!(tail.status(), 200);
    assert_eq!(
        tail.json::<Value>().await.unwrap()["events"]
            .as_array()
            .unwrap()
            .len(),
        5
    );

    // A token for another room opens nothing here.
    let elsewhere = replay(token_for("interview-def67890"), "").await;
    assert_eq!(elsewhere.status(), 404);

    // Forged and absent credentials are the same answer.
    let forged = codetrial::token::livekit_token(codetrial::token::LivekitTokenInput {
        api_key: "devkey",
        api_secret: "not-the-secret",
        identity: "EG_recorder",
        name: "recorder",
        room,
        metadata: "",
        agent: false,
        now_seconds: codetrial::current_epoch_seconds(),
    })
    .unwrap();
    assert_eq!(replay(forged, "").await.status(), 401);
    assert_eq!(
        client
            .get(format!("{base}/api/recording/replay"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// A refused start says which refusal it was.
///
/// Every branch here is a different thing for the candidate to do: join the
/// interview first, wait because the server has recording switched off, or
/// finish the recording already running. Nothing asserted any of them, so the
/// whole mapping could have collapsed to one answer and the page would have
/// shown the wrong instruction with the right status.
#[tokio::test]
async fn a_refused_recording_says_which_refusal_it_was() {
    let (base, server, path, client, cookie) = recorded_server("start-refusals").await;

    // No room yet, because nothing minted a token for this interview.
    let interview = start_interview(&client, &base, &cookie).await;
    let refused = client
        .post(format!("{base}/api/interviews/{interview}/recording"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 409);
    assert_eq!(
        refused.json::<Value>().await.unwrap()["code"],
        "recording_has_no_room",
        "an interview with no room is told to join first"
    );

    // An interview that is not this account's is refused as if it did not
    // exist, which is the same answer as consent withdrawn on purpose.
    let (other_cookie, _) = second_account(&client, &base, &path).await;
    let theirs = client
        .post(format!("{base}/api/interviews/{interview}/recording"))
        .header("cookie", other_cookie)
        .send()
        .await
        .unwrap();
    assert_ne!(
        theirs.status(),
        202,
        "another account may not start this recording"
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// Each webhook LiveKit sends does its own job, and says whether to resend.
///
/// The kinds were routed and acted on with nothing asserting either half. An
/// arm quietly dropped would leave the row where it was while the message was
/// answered 200, so LiveKit would never send it again and no test would notice
/// the recording had stopped moving.
///
/// The answer matters as much as the action. A `room_finished` for a room this
/// server does not know is finished business, but an egress event naming a job
/// nothing has written down yet is the only notice that job will ever get, so
/// it has to be given back rather than swallowed.
#[tokio::test]
async fn each_webhook_kind_moves_the_row_and_answers_for_itself() {
    // The fake provider, because ending a recording really calls StopEgress and
    // the real one answers a test server with 401.
    let provider = std::sync::Arc::new(FakeRecordingProvider::default());
    let (base, server, path, client, _cookie) =
        recorded_server_with_provider("webhook-kinds", provider.clone()).await;

    // Fresh, so the sweeper that started with the server leaves this alone.
    let now = codetrial::current_epoch_seconds_i64();
    let room = "interview-abc12345";
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO interviews (id, account_id, consent_version, consent_at, room_name)
               VALUES ('int-kinds', 1, '2026-08-21', ?1, ?2)",
            rusqlite::params![now, room],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO recordings (
                 id, account_id, interview_id, room_name, idempotency_key,
                 recipient_email, state, egress_id, created_at, updated_at
             ) VALUES ('rec-kinds', 1, 'int-kinds', ?2, 'idem-kinds',
                 'one@example.test', 'starting', 'EG_kinds', ?1, ?1)",
            rusqlite::params![now, room],
        )
        .unwrap();
    drop(connection);

    let state = || -> String {
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT state FROM recordings WHERE id = 'rec-kinds'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };

    let egress_started = json!({
        "event": "egress_started",
        "id": "EV_started",
        "createdAt": "1770000001",
        "egressInfo": { "egressId": "EG_kinds", "status": "EGRESS_ACTIVE" }
    })
    .to_string();
    assert_eq!(
        post_webhook(&client, &base, &egress_started).await.status(),
        200
    );
    assert_eq!(
        state(),
        "recording",
        "egress_started is what says the job is running"
    );

    let room_finished = json!({
        "event": "room_finished",
        "id": "EV_finished",
        "createdAt": "1770000002",
        "room": { "name": room }
    })
    .to_string();
    assert_eq!(
        post_webhook(&client, &base, &room_finished).await.status(),
        200
    );
    assert_eq!(
        state(),
        "finalizing",
        "the room ending is what ends the recording in it"
    );

    // A room this server never recorded. Nothing to do, and nothing to resend.
    let other_room = json!({
        "event": "room_finished",
        "id": "EV_unknown_room",
        "createdAt": "1770000003",
        "room": { "name": "interview-nosuchroom" }
    })
    .to_string();
    assert_eq!(
        post_webhook(&client, &base, &other_room).await.status(),
        200,
        "a room with no recording is finished business"
    );

    // An egress nothing has written down. The start response may still be in
    // flight, so this has to come back rather than be answered.
    let unknown_egress = json!({
        "event": "egress_ended",
        "id": "EV_unknown_egress",
        "createdAt": "1770000004",
        "egressInfo": { "egressId": "EG_never_seen", "status": "EGRESS_COMPLETE" }
    })
    .to_string();
    assert_eq!(
        post_webhook(&client, &base, &unknown_egress).await.status(),
        503,
        "an unknown egress is given back so LiveKit sends it again"
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// A full page is not the same as a page with more behind it.
///
/// The listing asks for one row more than a page and uses the extra row as the
/// answer to "is there another page". Only a single-row listing was covered, so
/// the comparison that decides it was free to be off by one: a cursor handed
/// out at exactly one page sends the client back for a page that is empty, and
/// a cursor withheld at one page more hides every recording past the twentieth.
#[tokio::test]
async fn a_full_page_offers_a_cursor_only_when_more_follows() {
    let (base, server, path, client, cookie) = recorded_server("listing-page-edge").await;

    // One recording per interview, which the schema enforces, so each row
    // brings its own.
    let insert = |count: i64| {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute("DELETE FROM recordings", []).unwrap();
        for row in 0..count {
            let interview_id = format!("int-page-{row}");
            let room = format!("interview-page{row:04}");

            // With its room, because an interview still waiting for one is
            // unique per account and consent version, and these are past.
            connection
                .execute(
                    "INSERT OR IGNORE INTO interviews
                       (id, account_id, consent_version, consent_at, room_name)
                       VALUES (?1, 1, '2026-08-21', 1, ?2)",
                    [&interview_id, &room],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO recordings (
                         id, account_id, interview_id, room_name, idempotency_key,
                         recipient_email, state, created_at, updated_at
                     ) VALUES (?1, 1, ?2, ?3, ?1, 'one@example.test', 'ready', ?4, ?4)",
                    rusqlite::params![format!("rec-page-{row}"), interview_id, room, 1000 + row],
                )
                .unwrap();
        }
    };
    let listing = || async {
        client
            .get(format!("{base}/api/recordings"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };

    let page = codetrial::recording::RECORDING_PAGE;
    insert(page);
    let body = listing().await;
    assert_eq!(body["recordings"].as_array().unwrap().len(), page as usize);
    assert_eq!(
        body["nextCursor"],
        Value::Null,
        "exactly one page has nothing behind it"
    );

    insert(page + 1);
    let body = listing().await;
    assert_eq!(
        body["recordings"].as_array().unwrap().len(),
        page as usize,
        "a page is still a page"
    );
    assert!(
        body["nextCursor"].is_string(),
        "one row more than a page is another page"
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// A signature from one project does not reach another project's room.
///
/// The signature proves which project sent the webhook. It does not prove the
/// project owns the room the webhook names, and a deployment with more than one
/// LiveKit project has both questions to answer. Without the second one, any
/// configured project could end another project's recording by naming its room,
/// and nothing here was asking it.
#[tokio::test]
async fn a_webhook_from_another_project_does_not_touch_the_room() {
    let (mut config, _cookie, path) = signed_in_web_config("webhook-other-project");

    // The recording override: a second key for the same project, which is what
    // it is for. LiveKit signs a webhook with whichever key the project's
    // webhook configuration names, and that need not be the key this server
    // makes Egress calls with, so both have to open this project's own rooms
    // and neither may open anybody else's.
    let mut recording = recording_config();
    recording.livekit = Some(codetrial::config::RecordingLivekit {
        url: "wss://example.livekit.cloud".to_string(),
        api_key: "override-key".to_string(),
        api_secret: "override-secret".to_string(),
    });
    config.recording = Some(recording);

    // Two projects, so the intruder's key verifies. The room routes to the
    // primary one, which is the project that owns this recording.
    config.pool.providers.push(provider("eu", "eu"));

    // Fresh, because the sweeper starts with the server and fails an active row
    // that has gone quiet. A recording reaped for being stale would look
    // exactly like one the intruder ended.
    let now = codetrial::current_epoch_seconds_i64();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO interviews (id, account_id, consent_version, consent_at)
               VALUES ('int-other', 1, '2026-08-21', ?1)",
            [now],
        )
        .unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO recordings (
                 id, account_id, interview_id, room_name, idempotency_key,
                 recipient_email, state, egress_id, created_at, updated_at
             ) VALUES ('rec-other', 1, 'int-other', 'interview-abc12345', 'idem-other',
                 'one@example.test', 'recording', 'EG_other', ?1, ?1)",
            [now],
        )
        .unwrap();

    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();

    let failed = json!({
        "event": "egress_ended",
        "id": "EV_other_project",
        "createdAt": "1770000123",
        "egressInfo": { "egressId": "EG_other", "status": "EGRESS_FAILED" }
    })
    .to_string();

    // Signed by the other project, correctly. The signature check passes and
    // the ownership check is the only thing between it and the row.
    let response = post_webhook_signed_by(&client, &base, &failed, "eu-key", b"eu-secret").await;
    assert_eq!(
        response.status(),
        200,
        "the message is answered rather than retried forever"
    );

    let state = || -> String {
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT state FROM recordings WHERE id = 'rec-other'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(
        state(),
        "recording",
        "another project's webhook must not end this recording"
    );

    // The override key, for this project's own room, is the case the override
    // exists for and it has to still work.
    let owned = json!({
        "event": "egress_ended",
        "id": "EV_own_project",
        "createdAt": "1770000456",
        "egressInfo": { "egressId": "EG_other", "status": "EGRESS_FAILED" }
    })
    .to_string();
    assert_eq!(
        post_webhook_signed_by(&client, &base, &owned, "override-key", b"override-secret")
            .await
            .status(),
        200
    );
    assert_eq!(
        state(),
        "failed",
        "the project's own second key opens its own room"
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// A webhook nobody signed moves nothing.
///
/// The route is unauthenticated by design -- LiveKit holds no session -- so the
/// signature is the whole of its access control, and it drives the recording
/// state machine for any room it can name. Every other webhook test in this
/// file signs correctly, so the handler could have stopped verifying entirely
/// and all of them would still pass; that was measured, not assumed.
///
/// Four refusals, because they are four different failures and each has its own
/// branch: no credential at all, a credential naming a project this server does
/// not know, a valid-looking one signed with the wrong secret, and one signed
/// correctly over different bytes. The last is what says the handler verifies
/// the body it received rather than something it has already parsed.
///
/// The accepted message at the end is what makes the four a check rather than a
/// server that refuses everything.
#[tokio::test]
async fn an_unsigned_webhook_cannot_move_a_recording() {
    let (mut config, _cookie, path) = signed_in_web_config("webhook-unsigned");
    config.recording = Some(recording_config());

    // Fresh, because the sweeper starts with the server and fails an active row
    // that has gone quiet. A recording reaped for being stale would look
    // exactly like one an unsigned webhook ended.
    let now = codetrial::current_epoch_seconds_i64();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(&format!(
            "INSERT INTO interviews (id, account_id, consent_version, consent_at, room_name)
               VALUES ('int-unsigned', 1, '2026-08-21', {now}, 'interview-abc12345');
             INSERT INTO recordings (
                 id, account_id, interview_id, room_name, idempotency_key,
                 recipient_email, state, egress_id, created_at, updated_at
             ) VALUES ('rec-unsigned', 1, 'int-unsigned', 'interview-abc12345', 'idem-unsigned',
                 'one@example.test', 'recording', 'EG_unsigned', {now}, {now});"
        ))
        .unwrap();

    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();
    let state = || -> String {
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT state FROM recordings WHERE id = 'rec-unsigned'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };

    let failed = json!({
        "event": "egress_ended",
        "id": "EV_unsigned",
        "createdAt": "1770000123",
        "egressInfo": { "egressId": "EG_unsigned", "status": "EGRESS_FAILED" }
    })
    .to_string();

    // A body this server would act on if it were signed, so that anything
    // rejected below is rejected by the credential and not by the message.
    let decoy = json!({
        "event": "egress_ended",
        "id": "EV_decoy",
        "createdAt": "1770000123",
        "egressInfo": { "egressId": "EG_unsigned", "status": "EGRESS_COMPLETE" }
    })
    .to_string();

    for (why, authorization) in [
        ("no credential at all", String::new()),
        ("a bearer that is not a token", "not-a-jwt".to_string()),
        (
            "a project this server has never heard of",
            webhook_authorization(&failed, "someone-elses-key", b"devsecret"),
        ),
        (
            "the right project and the wrong secret",
            webhook_authorization(&failed, "devkey", b"not-devsecret"),
        ),
        (
            "a signature over a different body",
            webhook_authorization(&decoy, "devkey", b"devsecret"),
        ),
    ] {
        let refused = post_webhook_authorized_by(&client, &base, &failed, &authorization).await;
        assert_eq!(refused.status(), 401, "{why}");
        assert_eq!(
            refused.json::<Value>().await.unwrap()["error"],
            "webhook_signature_invalid",
            "{why}"
        );
        assert_eq!(state(), "recording", "{why} moved the row");
    }

    // The same bytes, signed by the project that owns the room.
    assert_eq!(post_webhook(&client, &base, &failed).await.status(), 200);
    assert_eq!(
        state(),
        "failed",
        "a correctly signed webhook is still acted on"
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// The server starts the recording workers it is configured for.
///
/// Nothing checked that it does. `web_router` decides whether to spawn them
/// from the accounts handle and the recorder, and a build that quietly stopped
/// would serve every request correctly while no recording was ever swept,
/// delivered or expired. The kill switch makes that observable in a test: it
/// takes every active row on the first pass, and the first pass runs when the
/// worker starts rather than a minute later.
#[tokio::test]
async fn the_server_starts_the_recording_workers() {
    let (mut config, _cookie, path) = signed_in_web_config("worker-start");
    config.recording = Some(codetrial::config::RecordingConfig {
        kill_switch: true,
        ..recording_config()
    });

    // Written before the server exists, because the sweep this asserts on is
    // the one the worker runs as it starts.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "INSERT INTO interviews (id, account_id, consent_version, consent_at)
               VALUES ('int-worker', 1, '2026-08-21', 1);
             INSERT INTO recordings (
                 id, account_id, interview_id, room_name, idempotency_key,
                 recipient_email, state, created_at, updated_at
             ) VALUES ('rec-worker', 1, 'int-worker', 'interview-worker1', 'idem-worker',
                 'one@example.test', 'recording', 1, 1);",
        )
        .unwrap();

    let (_base, server) = spawn_web_server(config).await;

    let state = || {
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT state FROM recordings WHERE id = 'rec-worker'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    };
    for _ in 0..100 {
        if state() == "failed" {
            server.shutdown().await;
            remove_database(path).await;
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let reached = state();
    server.shutdown().await;
    remove_database(path).await;
    panic!("the sweeper never ran: the recording is still {reached}");
}

/// A storage failure is not the same answer as no such recording.
///
/// This route is what the recording template asks before it decides a replay
/// exists. A read that failed used to arrive as `404`, which tells the template
/// the recording is gone: a permanent answer to a temporary condition, and one
/// no caller retries.
#[tokio::test]
async fn a_replay_read_that_fails_is_not_reported_as_missing() {
    let (base, server, path, client, cookie) = recorded_server("replay-read-error").await;
    let interview = start_interview(&client, &base, &cookie).await;
    let room = "interview-abc12345";
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        ) VALUES ('rec-err', 1, ?1, ?2, ?1, 'one@example.test', 'recording', 1, 1)
        ",
            [&interview, &room.to_string()],
        )
        .unwrap();
    let token = codetrial::token::livekit_token(codetrial::token::LivekitTokenInput {
        api_key: "devkey",
        api_secret: "devsecret",
        identity: "EG_recorder",
        name: "recorder",
        room,
        metadata: "",
        agent: false,
        now_seconds: codetrial::current_epoch_seconds(),
    })
    .unwrap();
    let replay = |token: String| {
        let url = format!("{base}/api/recording/replay");
        let client = client.clone();
        async move {
            client
                .get(url)
                .header("authorization", token)
                .send()
                .await
                .unwrap()
        }
    };

    assert_eq!(
        replay(token.clone()).await.status(),
        200,
        "the room this token names has a recording to read"
    );

    // The table out from under the read, which is the shape a storage failure
    // takes here: the query errors rather than returning no rows.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TABLE recordings")
        .unwrap();

    let broken = replay(token).await;
    assert_eq!(
        broken.status(),
        500,
        "a failed read is not the answer 'no such recording'"
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// The history a candidate reads about their own interviews.
#[tokio::test]
async fn history_lists_own_recordings_only() {
    let (base, server, path, client, cookie) = recorded_server("history-lists").await;
    let interview = start_interview(&client, &base, &cookie).await;
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "INSERT INTO users (id, github_id, login, email, email_verified, created_at, updated_at)
                   VALUES (2, 202, 'two', 'two@example.test', 1, 1, 1);
                 INSERT INTO interviews (id, account_id, consent_version, consent_at)
                   VALUES ('int-theirs', 2, '2026-08-21', 1);",
            )
            .unwrap();
        connection
            .execute(
                "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at, ready_at, expires_at
        ) VALUES ('rec-mine', 1, ?1, 'interview-abc12345', ?1,
            'one@example.test', 'ready', 10, 10, 12, 99999999999)
        ",
                [&interview],
            )
            .unwrap();
        connection
            .execute(
                "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        ) VALUES ('rec-theirs', 2, 'int-theirs', 'interview-def67890', 'int-theirs',
            'two@example.test', 'ready', 11, 11)
        ",
                [],
            )
            .unwrap();
    }

    let listed = client
        .get(format!("{base}/api/recordings"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(listed.status(), 200);
    let body = listed.json::<Value>().await.unwrap();
    assert_eq!(
        body["recordings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["recordingId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["rec-mine"],
        "somebody else's interview is not in this account's history"
    );
    assert_eq!(body["nextCursor"], Value::Null, "one page holds one row");

    // The detail carries no handle to media: not the object path, not the Drive
    // file, not the permission.
    let detail = client
        .get(format!("{base}/api/recordings/rec-mine"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(detail.status(), 200);
    let detail = detail.json::<Value>().await.unwrap();
    assert_eq!(detail["state"], "ready");
    assert_eq!(detail["readyAt"], 12);
    let printed = detail.to_string();
    for handle in ["gcsObject", "driveFileId", "drivePermissionId", "roomName"] {
        assert!(
            !printed.contains(handle),
            "{handle} is a handle to media: {printed}"
        );
    }

    server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn history_cross_account_denied() {
    let (base, server, path, client, cookie) = recorded_server("history-cross").await;
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "INSERT INTO users (id, github_id, login, email, email_verified, created_at, updated_at)
                   VALUES (2, 202, 'two', 'two@example.test', 1, 1, 1);
                 INSERT INTO interviews (id, account_id, consent_version, consent_at)
                   VALUES ('int-theirs', 2, '2026-08-21', 1);
                 INSERT INTO recordings (
                     id, account_id, interview_id, room_name, idempotency_key,
                     recipient_email, state, created_at, updated_at
                 ) VALUES ('rec-theirs', 2, 'int-theirs', 'interview-def67890', 'int-theirs',
                     'two@example.test', 'ready', 11, 11);
                 INSERT INTO replay_events (interview_id, seq, kind, at, payload, bytes, received_at)
                   VALUES ('int-theirs', 0, 'transcript', 1, '{}', 2, 1);",
            )
            .unwrap();
    }

    // Not `403`: an account that does not own a recording should not learn that
    // it exists. The two paths are refused by two different scopings, which is
    // the point of asking both: the detail is scoped by `recording_summary`'s
    // `account_id`, and the events by the replay read's own ownership check.
    for path_suffix in ["", "/events", "/events?avatar=history"] {
        let response = client
            .get(format!("{base}/api/recordings/rec-theirs{path_suffix}"))
            .header("cookie", cookie.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404, "{path_suffix}");
    }
    let signed_out = client
        .get(format!("{base}/api/recordings"))
        .send()
        .await
        .unwrap();
    assert_eq!(signed_out.status(), 401);

    // A cursor that does not parse is refused rather than read as "start
    // again", which would hand back the first page and look like a list that
    // repeats itself.
    let nonsense = client
        .get(format!("{base}/api/recordings?before=not-a-cursor"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(nonsense.status(), 400);
    assert_eq!(
        nonsense.json::<Value>().await.unwrap()["code"],
        "cursor_invalid"
    );

    server.shutdown().await;
    remove_database(path).await;
}

#[tokio::test]
async fn history_expired_returns_410() {
    let (base, server, path, client, cookie) = recorded_server("history-expired").await;
    let interview = start_interview(&client, &base, &cookie).await;
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at, expires_at
        ) VALUES ('rec-expired', 1, ?1, 'interview-abc12345', ?1,
            'one@example.test', 'ready', 10, 10, 1)
        ",
                [&interview],
            )
            .unwrap();
    }

    // Gone rather than missing: this account owns the interview and is owed the
    // difference between "never yours" and "not any more".
    //
    // `?avatar=history` is in this list to pin where the refusal happens, not
    // to re-prove it. The parameter is parsed before `gone_response` and used
    // after it, so what fails here is an edit that moves the read itself in
    // front of the guard. That the review read refuses on its own, guard or no
    // guard, is `replay_review_is_gone_when_the_snapshot_is` in
    // `tests/recording.rs`, which calls it directly.
    for path_suffix in ["", "/events", "/events?avatar=history"] {
        let response = client
            .get(format!("{base}/api/recordings/rec-expired{path_suffix}"))
            .header("cookie", cookie.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 410, "{path_suffix}");
        assert_eq!(
            response.json::<Value>().await.unwrap()["code"],
            "replay_expired"
        );
    }

    // A deleted recording says so in its own words, because the two are
    // different things to a person: one waited too long, the other was taken
    // away.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "
        UPDATE recordings SET state = 'deleted', deleted_at = 5, expires_at = NULL,
            room_name = NULL, recipient_email = NULL WHERE id = 'rec-expired'
        ",
            [],
        )
        .unwrap();
    for path_suffix in ["", "/events", "/events?avatar=history"] {
        let deleted = client
            .get(format!("{base}/api/recordings/rec-expired{path_suffix}"))
            .header("cookie", cookie.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(deleted.status(), 410, "{path_suffix}");
        assert_eq!(
            deleted.json::<Value>().await.unwrap()["code"],
            "recording_deleted",
            "{path_suffix}"
        );
    }

    server.shutdown().await;
    remove_database(path).await;
}

/// The review page's read: every `avatar` row, and one editor buffer.
///
/// The snapshot keeps the newest row of every replaceable kind, which is what a
/// recording template joining late needs and exactly what a response window
/// cannot be computed from. `?avatar=history` lifts the collapse on that one
/// kind and on no other, so the page pays for the state history it reads rather
/// than for a code buffer per debounce.
#[tokio::test]
async fn history_events_avatar_history_returns_every_state() {
    let (base, server, path, client, cookie) = recorded_server("history-avatar").await;
    let interview = start_interview(&client, &base, &cookie).await;
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        ) VALUES ('rec-window', 1, ?1, 'interview-abc12345', ?1,
            'one@example.test', 'ready', 10, 10)
        ",
                [&interview],
            )
            .unwrap();
    }

    let posted = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [

            // The states the server publishes, so this is a replay a real
            // interview could produce: `src/livekit.rs` writes `listening` and
            // `speaking` and nothing writes `thinking`.
            envelope("avatar", json!({ "state": "speaking" })),
            envelope("editor", json!({ "code": "first draft" })),
            envelope("transcript", json!({ "speaker": "you", "text": "a hash map" })),
            envelope("avatar", json!({ "state": "listening" })),
            envelope("editor", json!({ "code": "second draft" })),
            envelope("lifecycle", json!({ "state": "paused" })),
            envelope("avatar", json!({ "state": "speaking" })),
        ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), 200);

    let rows = async |query: &str| {
        let response = client
            .get(format!("{base}/api/recordings/rec-window/events{query}"))
            .header("cookie", cookie.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{query}");
        response.json::<Value>().await.unwrap()["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| {
                (
                    event["seq"].as_i64().unwrap(),
                    event["kind"].as_str().unwrap().to_string(),
                )
            })
            .collect::<Vec<_>>()
    };

    let snapshot = rows("").await;
    assert_eq!(
        snapshot,
        vec![
            (2, "transcript".to_string()),
            (4, "editor".to_string()),
            (5, "lifecycle".to_string()),
            (6, "avatar".to_string()),
        ],
        "the default read is the snapshot the recording template also takes"
    );
    assert_eq!(
        rows("?avatar=history").await,
        vec![
            (0, "avatar".to_string()),
            (2, "transcript".to_string()),
            (3, "avatar".to_string()),
            (4, "editor".to_string()),
            (5, "lifecycle".to_string()),
            (6, "avatar".to_string()),
        ],
        "the page read keeps every transition, seq 0 included, and still one editor"
    );

    // Exact, not truthy. A value nobody wrote is the ordinary snapshot rather
    // than a guess at what the caller meant. Compared against the whole default
    // answer rather than against its length: three rows of the wrong kinds is
    // also three rows.
    assert_eq!(
        rows("?avatar=latest").await,
        snapshot,
        "an unrecognised value reads the snapshot, not every avatar row"
    );

    // The tail collapses nothing already, so the parameter has nothing to lift
    // there and the request is answered as the tail it asked for.
    assert_eq!(
        rows("?after=0&avatar=history").await,
        vec![
            (1, "editor".to_string()),
            (2, "transcript".to_string()),
            (3, "avatar".to_string()),
            (4, "editor".to_string()),
            (5, "lifecycle".to_string()),
            (6, "avatar".to_string()),
        ],
        "and the tail is still the tail, which is why the page does not use it"
    );

    server.shutdown().await;
    remove_database(path).await;
}

/// A late join reads the snapshot, and only its own.
#[tokio::test]
async fn replay_snapshot_is_owner_scoped() {
    let (base, server, path, client, cookie) = recorded_server("replay-snapshot").await;
    let interview = start_interview(&client, &base, &cookie).await;
    let event = |kind: &str, text: &str| envelope(kind, json!({ "text": text }));
    let posted = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [
            event("editor", "first draft"),
            event("transcript", "hello"),
            event("editor", "second draft"),
        ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), 200);

    let snapshot = client
        .get(format!("{base}/api/interviews/{interview}/snapshot"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(snapshot.status(), 200);
    let body = snapshot.json::<Value>().await.unwrap();
    assert_eq!(body["seq"], 2, "the last event the snapshot accounts for");
    assert_eq!(body["quotaExceeded"], false);
    assert_eq!(
        body["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| (
                event["seq"].as_i64().unwrap(),
                event["kind"].as_str().unwrap()
            ))
            .collect::<Vec<_>>(),
        vec![(1, "transcript"), (2, "editor")],
        "the superseded editor snapshot is dropped and the transcript line is not"
    );

    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "INSERT INTO users (id, github_id, login, email, email_verified, created_at, updated_at)
                   VALUES (2, 202, 'two', 'two@example.test', 1, 1, 1);
                 INSERT INTO interviews (id, account_id, consent_version, consent_at)
                   VALUES ('int-theirs', 2, '2026-08-21', 1);",
            )
            .unwrap();
    }
    let theirs = client
        .get(format!("{base}/api/interviews/int-theirs/snapshot"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(theirs.status(), 404);

    // Past the retention deadline the replay is gone rather than missing: this
    // account owns the interview and is owed the difference.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, expires_at, created_at, updated_at
        ) VALUES ('rec-snap', 1, ?1, 'interview-abc12345', ?1,
            'one@example.test', 'ready', 1, 1, 1)
        ",
            [&interview],
        )
        .unwrap();
    let expired = client
        .get(format!("{base}/api/interviews/{interview}/snapshot"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status(), 410);
    assert_eq!(
        expired.json::<Value>().await.unwrap()["code"],
        "replay_expired"
    );

    // And a deleted recording, which is the other half of the same promise and
    // was asserted nowhere: this route has no `gone_response` ahead of it, so
    // `SnapshotView::Deleted` reaches the response mapping directly here. The
    // status was free to become a `404` without a test noticing, which is the
    // difference between "taken away" and "never yours" going missing.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "
        UPDATE recordings SET state = 'deleted', deleted_at = 5, expires_at = NULL,
            room_name = NULL, recipient_email = NULL WHERE id = 'rec-snap'
        ",
            [],
        )
        .unwrap();
    let deleted = client
        .get(format!("{base}/api/interviews/{interview}/snapshot"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 410);
    assert_eq!(
        deleted.json::<Value>().await.unwrap()["code"],
        "recording_deleted"
    );

    let signed_out = client
        .get(format!("{base}/api/interviews/{interview}/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(signed_out.status(), 401);

    server.shutdown().await;
    remove_database(path).await;
}

/// Ending an interview that recorded nothing says so, rather than failing.
///
/// 204 and no body: the interview ending is the candidate's news whether or
/// not a recording was running, so there is nothing to report and nothing to
/// stop. A 404 would tell them the interview was never theirs.
#[tokio::test]
async fn ending_an_interview_without_a_recording_is_a_no_op() {
    let (base, server, path, client, cookie) = recorded_server("end-no-recording").await;
    let interview = start_interview(&client, &base, &cookie).await;

    let ended = client
        .post(format!("{base}/api/interviews/{interview}/end"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(ended.status(), 204);
    assert!(ended.bytes().await.unwrap().is_empty());

    // Idempotent, because the browser sends this on unload and on the button.
    let again = client
        .post(format!("{base}/api/interviews/{interview}/end"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 204);

    server.shutdown().await;
    remove_database(path).await;
}

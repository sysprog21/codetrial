//! How the token route treats the starts an account holds that no candidate
//! has joined.

use super::*;

/// The dispatcher bounds what one account may leave waiting, and the route
/// says so to the candidate in terms they can act on. 429, not 503: the server
/// has room, and this account is the one over its share.
#[tokio::test]
async fn an_account_over_its_waiting_starts_is_told_to_join_or_close_one() {
    let (mut config, cookie, db_path) = signed_in_web_config("dispatch-waiting");
    config.production = true;
    config.fixed_room_name = None;
    let dispatcher = std::sync::Arc::<RecordingDispatcher>::default();
    dispatcher
        .too_many_waiting
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let (base, server) =
        spawn_web_server_with_dispatcher(config, std::sync::Arc::clone(&dispatcher)).await;

    let response = http_client()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .json(&json!({"problemId":"two-sum","durationMin":45}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 429);
    let body: Value = response.json().await.unwrap();
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("waiting to start"), "{body}");
    assert!(
        !error.contains("as many interviews"),
        "not a full server: {body}"
    );
    assert!(body.get("token").is_none(), "a refused start has no token");

    server.shutdown().await;
    remove_database(db_path).await;
}

/// The count is by account, so the route has to name the account on every
/// signed-in start. It used to be read off the recorded consent, which a
/// deployment that does not record has none of, so its accounts were never
/// counted.
#[tokio::test]
async fn a_signed_in_start_names_its_account_to_the_dispatcher() {
    let (mut config, cookie, db_path) = signed_in_web_config("dispatch-owner");
    config.production = true;
    config.fixed_room_name = None;
    let dispatcher = std::sync::Arc::<RecordingDispatcher>::default();
    let (base, server) =
        spawn_web_server_with_dispatcher(config, std::sync::Arc::clone(&dispatcher)).await;

    let response = http_client()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .json(&json!({"problemId":"two-sum","durationMin":45}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    let owners = dispatcher.owners.lock().unwrap().clone();
    assert_eq!(owners.len(), 1);
    assert!(
        owners[0].is_some(),
        "the start was made by an account: {owners:?}"
    );

    server.shutdown().await;
    remove_database(db_path).await;
}

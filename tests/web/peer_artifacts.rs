use super::*;

#[tokio::test]
async fn saved_audio_download_and_withdrawal_are_member_scoped_and_expiry_blocks_reads() {
    let audio_dir = std::env::temp_dir().join(format!(
        "codetrial-peer-http-audio-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let configured_dir = audio_dir.clone();
    let f = PeerFixture::configured("peer-audio-access", move |config| {
        config.web_dir = configured_dir.with_extension("unserved-web");
        config.peer_observer =
            codetrial::config::load_peer_observer(&std::collections::BTreeMap::from([
                ("GOOGLE_API_KEY".into(), "audio-fixture-key".into()),
                (
                    "CODETRIAL_PEER_AUDIO_DIR".into(),
                    configured_dir.to_string_lossy().into_owned(),
                ),
            ]))
            .unwrap();
    })
    .await;
    let created = f
        .post("/api/peer/sessions", 0, json!({"saveAudio":true}))
        .await;
    let id = created["sessionId"].as_str().unwrap();
    f.post(
        "/api/peer/invites/accept",
        1,
        json!({"token":created["inviteToken"]}),
    )
    .await;
    let bytes = b"RIFFsynthetic-private-audio";
    let audio_file = audio_dir.join(format!("{id}.wav"));
    std::fs::write(&audio_file, bytes).unwrap();
    let now = i64::try_from(codetrial::web::current_epoch_seconds()).unwrap();
    let connection = rusqlite::Connection::open(&f.path).unwrap();
    connection.execute("INSERT INTO peer_audio_records(session_id,status,bytes,duration_ms,created_at,expires_at) VALUES(?1,'ready',?2,100,?3,?4)",rusqlite::params![id,i64::try_from(bytes.len()).unwrap(),now,now+86400]).unwrap();
    let route = format!("/api/peer/sessions/{id}/audio");
    assert_eq!(
        f.request(reqwest::Method::GET, &route, 2, Value::Null)
            .await
            .status(),
        404
    );
    assert_eq!(
        f.request(reqwest::Method::DELETE, &route, 2, json!({}))
            .await
            .status(),
        404
    );
    let download = f
        .request(reqwest::Method::GET, &route, 1, Value::Null)
        .await;
    assert_eq!(download.status(), 200);
    assert_eq!(download.headers()["content-type"], "audio/wav");
    assert_eq!(download.headers()["cache-control"], "no-store");
    assert_eq!(download.bytes().await.unwrap().as_ref(), bytes);
    connection
        .execute(
            "UPDATE peer_audio_records SET expires_at=?2 WHERE session_id=?1",
            rusqlite::params![id, now - 1],
        )
        .unwrap();
    assert_eq!(
        f.request(reqwest::Method::GET, &route, 0, Value::Null)
            .await
            .status(),
        410
    );
    let status = f.get(&format!("{route}/status"), 0).await;
    assert_eq!(status["status"], "deleted");
    let deleted = f
        .request(reqwest::Method::DELETE, &route, 1, json!({}))
        .await;
    assert_eq!(deleted.status(), 200);
    assert!(!audio_file.exists());
    drop(connection);
    f.close().await;
    std::fs::remove_dir_all(audio_dir).unwrap();
}

#[tokio::test]
async fn suggestions_are_end_only_and_each_member_sees_only_their_own() {
    let f = PeerFixture::new("peer-advice-access").await;
    let (path, created) = f.create().await;
    f.post(
        "/api/peer/invites/accept",
        1,
        json!({"token":created["inviteToken"]}),
    )
    .await;
    let id = created["sessionId"].as_str().unwrap();
    let now = i64::try_from(codetrial::web::current_epoch_seconds()).unwrap();
    let connection = rusqlite::Connection::open(&f.path).unwrap();
    connection.execute("INSERT INTO peer_observations(session_id,status,advice_json,created_at,completed_at,last_seen_at,expires_at) VALUES(?1,'ready',?2,?3,?3,?3,?4)",rusqlite::params![id,json!([{"slot":0,"summary":"Advice for A"},{"slot":1,"summary":"Advice for B"}]).to_string(),now,now+86400]).unwrap();
    assert!(
        f.get(&format!("{path}/advice"), 0).await["advice"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let state = f.get(&path, 0).await;
    f.post(
        &format!("{path}/terminate"),
        0,
        json!({"expectedVersion":state["version"]}),
    )
    .await;
    for member in 0..2 {
        let advice = f.get(&format!("{path}/advice"), member).await;
        let visible = advice["advice"].as_array().unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0]["slot"], member);
    }
    assert_eq!(
        f.request(
            reqwest::Method::GET,
            &format!("{path}/advice"),
            2,
            Value::Null
        )
        .await
        .status(),
        404
    );
    assert_eq!(
        f.request(
            reqwest::Method::GET,
            &format!("{path}/transcript"),
            0,
            Value::Null
        )
        .await
        .status(),
        404
    );
    connection
        .execute(
            "UPDATE peer_observations SET expires_at=?2 WHERE session_id=?1",
            rusqlite::params![id, now - 1],
        )
        .unwrap();
    let expired = f
        .request(
            reqwest::Method::GET,
            &format!("{path}/advice"),
            0,
            Value::Null,
        )
        .await;
    assert_eq!(expired.status(), 404);
    drop(connection);
    f.close().await;
}

#[tokio::test]
async fn embedded_server_refuses_audio_storage_beneath_its_public_web_root() {
    let root = std::env::temp_dir().join(format!(
        "codetrial-public-audio-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let configured = root.clone();
    let f = PeerFixture::configured("peer-public-audio", move |config| {
        config.web_dir = configured.clone();
        config.peer_observer =
            codetrial::config::load_peer_observer(&std::collections::BTreeMap::from([
                ("GOOGLE_API_KEY".into(), "private-audio-fixture".into()),
                (
                    "CODETRIAL_PEER_AUDIO_DIR".into(),
                    configured.join("audio").to_string_lossy().into_owned(),
                ),
            ]))
            .unwrap();
    })
    .await;
    assert_eq!(f.get("/api/session", 0).await["peerAudioAvailable"], false);
    let response = f
        .request(
            reqwest::Method::POST,
            "/api/peer/sessions",
            0,
            json!({"saveAudio":true}),
        )
        .await;
    assert_eq!(response.status(), 503);
    f.close().await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn audio_that_was_not_chosen_is_reported_unavailable_rather_than_empty() {
    let f = PeerFixture::new("peer-audio-unchosen").await;
    let (path, _) = f.create().await;
    let status = f.get(&format!("{path}/audio/status"), 0).await;
    assert_eq!(status["status"], "unavailable");
    let download = f
        .request(
            reqwest::Method::GET,
            &format!("{path}/audio"),
            0,
            Value::Null,
        )
        .await;
    assert_eq!(download.status(), 404);
    f.close().await;
}

#[tokio::test]
async fn a_withdrawal_that_cannot_unlink_the_file_is_reported_as_pending() {
    let audio_dir = std::env::temp_dir().join(format!(
        "codetrial-peer-http-unlink-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let configured_dir = audio_dir.clone();
    let f = PeerFixture::configured("peer-audio-pending", move |config| {
        config.web_dir = configured_dir.with_extension("unserved-web");
        config.peer_observer =
            codetrial::config::load_peer_observer(&std::collections::BTreeMap::from([
                ("GOOGLE_API_KEY".into(), "audio-fixture-key".into()),
                (
                    "CODETRIAL_PEER_AUDIO_DIR".into(),
                    configured_dir.to_string_lossy().into_owned(),
                ),
            ]))
            .unwrap();
    })
    .await;
    let created = f
        .post("/api/peer/sessions", 0, json!({"saveAudio":true}))
        .await;
    let id = created["sessionId"].as_str().unwrap();
    // A directory cannot be unlinked as a file, so deletion has to be retried.
    std::fs::create_dir_all(audio_dir.join(format!("{id}.wav"))).unwrap();
    let route = format!("/api/peer/sessions/{id}/audio");
    let response = f
        .request(reqwest::Method::DELETE, &route, 0, json!({}))
        .await;
    error(response, 503, "audio_delete_pending").await;
    f.close().await;
    std::fs::remove_dir_all(audio_dir).unwrap();
}

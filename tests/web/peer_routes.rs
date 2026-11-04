use super::*;

fn session_row(f: &PeerFixture, path: &str, column: &str) -> i64 {
    let id = path.rsplit('/').next().unwrap();
    rusqlite::Connection::open(&f.path)
        .unwrap()
        .query_row(
            &format!("SELECT {column} FROM peer_sessions WHERE id=?1"),
            [id],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn peer_body_ceilings_are_exact() {
    assert_eq!(codetrial::web::MAX_PEER_SNAPSHOT_BYTES, 2_097_152);
    assert_eq!(codetrial::web::MAX_PEER_FEEDBACK_BYTES, 262_144);
}

#[tokio::test]
async fn a_peer_origin_with_a_path_is_refused() {
    let f = PeerFixture::new("peer-origin-path").await;
    let response = f
        .client
        .post(format!("{}/api/peer/sessions", f.base))
        .header("cookie", &f.cookies[0])
        .header("origin", format!("{}/elsewhere", f.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    error(response, 403, "origin_mismatch").await;
    f.close().await;
}

#[tokio::test]
async fn starting_needs_a_post_that_carries_observer_consent() {
    let f = PeerFixture::new("peer-start-method").await;
    let path = f.joined().await;
    for member in 0..2 {
        f.post(&format!("{path}/enter"), member, json!({})).await;
    }
    let state = f.get(&path, 0).await;
    error(
        f.request(
            reqwest::Method::GET,
            &format!("{path}/start"),
            0,
            Value::Null,
        )
        .await,
        404,
        "not_found",
    )
    .await;
    error(
        f.request(
            reqwest::Method::POST,
            &format!("{path}/start"),
            0,
            json!({"expectedVersion":state["version"]}),
        )
        .await,
        403,
        "observer_consent_required",
    )
    .await;
    f.close().await;
}

#[tokio::test]
async fn entering_bumps_the_version_and_binds_the_first_room_generation() {
    let f = PeerFixture::new("peer-enter-generation").await;
    let path = f.joined().await;
    let before = f.get(&path, 0).await["version"].as_u64().unwrap();
    assert_eq!(session_row(&f, &path, "room_generation"), 0);
    let entered = f.post(&format!("{path}/enter"), 0, json!({})).await;
    assert_eq!(entered["version"].as_u64().unwrap(), before + 1);
    assert_eq!(session_row(&f, &path, "room_generation"), 1);
    let second = f.post(&format!("{path}/enter"), 1, json!({})).await;
    assert_eq!(second["version"].as_u64().unwrap(), before + 2);
    assert_eq!(session_row(&f, &path, "room_generation"), 1);
    f.close().await;
}

#[tokio::test]
async fn an_invite_is_only_for_a_pending_session_with_its_creator_alone() {
    let f = PeerFixture::new("peer-invite-guard").await;
    let (path, created) = f.create().await;
    let version = created["version"].clone();
    let renewed = f
        .post(
            &format!("{path}/invite"),
            0,
            json!({"expectedVersion":version}),
        )
        .await;
    assert!(renewed["inviteToken"].is_string());
    assert_eq!(
        renewed["version"].as_u64().unwrap(),
        created["version"].as_u64().unwrap() + 1
    );
    let revoked = f
        .request(
            reqwest::Method::DELETE,
            &format!("{path}/invite"),
            0,
            json!({"expectedVersion":renewed["version"]}),
        )
        .await;
    assert!(revoked.status().is_success());
    let revoked: Value = revoked.json().await.unwrap();
    assert_eq!(
        revoked["version"].as_u64().unwrap(),
        renewed["version"].as_u64().unwrap() + 1
    );
    let renewed = f
        .post(
            &format!("{path}/invite"),
            0,
            json!({"expectedVersion":revoked["version"]}),
        )
        .await;
    f.post(
        "/api/peer/invites/accept",
        1,
        json!({"token":renewed["inviteToken"]}),
    )
    .await;
    let joined = f.get(&path, 0).await;
    error(
        f.request(
            reqwest::Method::POST,
            &format!("{path}/invite"),
            0,
            json!({"expectedVersion":joined["version"]}),
        )
        .await,
        410,
        "invite_unavailable",
    )
    .await;
    error(
        f.request(
            reqwest::Method::POST,
            &format!("{path}/invite"),
            1,
            json!({"expectedVersion":joined["version"]}),
        )
        .await,
        410,
        "invite_unavailable",
    )
    .await;

    let (second, _) = f.create().await;
    let state = f.get(&second, 0).await;
    f.post(
        &format!("{second}/cancel"),
        0,
        json!({"expectedVersion":state["version"]}),
    )
    .await;
    let canceled = f.get(&second, 0).await;
    error(
        f.request(
            reqwest::Method::POST,
            &format!("{second}/invite"),
            0,
            json!({"expectedVersion":canceled["version"]}),
        )
        .await,
        410,
        "invite_unavailable",
    )
    .await;
    f.close().await;
}

#[tokio::test]
async fn a_session_action_with_extra_path_segments_is_not_found_and_changes_nothing() {
    let f = PeerFixture::new("peer-action-shape").await;
    let path = f.joined().await;
    let before = f.get(&path, 0).await;
    let extra = f
        .request(
            reqwest::Method::POST,
            &format!("{path}/ready/a/b"),
            0,
            json!({"expectedVersion":before["version"],"ready":true}),
        )
        .await;
    error(extra, 404, "not_found").await;
    assert_eq!(f.get(&path, 0).await, before);
    let bare = f
        .request(
            reqwest::Method::POST,
            &format!("{path}/rounds/0"),
            0,
            json!({}),
        )
        .await;
    error(bare, 404, "not_found").await;
    for tail in ["rounds", "rounds/0/writer/extra"] {
        let long = f
            .request(
                reqwest::Method::POST,
                &format!("{path}/{tail}"),
                0,
                json!({}),
            )
            .await;
        error(long, 404, "not_found").await;
    }
    assert_eq!(f.get(&path, 0).await, before);
    f.close().await;
}

#[tokio::test]
async fn a_round_snapshot_can_be_read_back_by_either_member() {
    let f = PeerFixture::new("peer-snapshot-read").await;
    let path = f.active().await;
    let writer = f
        .post(&format!("{path}/rounds/0/writer"), 1, json!({}))
        .await;
    f.request(
        reqwest::Method::PUT,
        &format!("{path}/rounds/0/snapshot"),
        1,
        json!({"lease":writer["lease"],"expectedRevision":0,"language":"python","code":"print(7)","tests":null,"final":false}),
    )
    .await;
    for member in 0..2 {
        let read = f.get(&format!("{path}/rounds/0/snapshot"), member).await;
        assert_eq!(read["code"], "print(7)");
        assert_eq!(read["revision"], 1);
    }
    f.close().await;
}

#[tokio::test]
async fn paths_outside_the_session_routes_are_not_served_as_sessions() {
    let f = PeerFixture::new("peer-path-shapes").await;
    let (path, _) = f.create().await;
    let id = path.rsplit('/').next().unwrap();
    let kept = f
        .post("/api/peer/sessions", 1, json!({"saveTranscript":true}))
        .await;
    let saved = format!("/api/peer/sessions/{}", kept["sessionId"].as_str().unwrap());
    for (method, route) in [
        (reqwest::Method::DELETE, "/api/peer/sessions".to_owned()),
        (reqwest::Method::GET, format!("/api/peer/invites/{id}")),
        (reqwest::Method::DELETE, format!("{path}/transcript/extra")),
        (reqwest::Method::GET, format!("{path}/audio/other")),
    ] {
        error(
            f.request(method, &route, 0, Value::Null).await,
            404,
            "not_found",
        )
        .await;
    }
    let withdrawn = f
        .request(
            reqwest::Method::DELETE,
            &format!("{saved}/transcript"),
            1,
            Value::Null,
        )
        .await;
    assert!(withdrawn.status().is_success());
    f.close().await;
}

#[tokio::test]
async fn only_a_delete_withdraws_a_transcript_and_only_a_get_reads_audio_status() {
    let f = PeerFixture::new("peer-method-shapes").await;
    let (path, _) = f.create().await;
    let nonsense = f
        .request(
            reqwest::Method::POST,
            &format!("{path}/nonsense"),
            0,
            json!({}),
        )
        .await;
    let expected = (nonsense.status(), nonsense.json::<Value>().await.unwrap());
    let response = f
        .request(
            reqwest::Method::POST,
            &format!("{path}/transcript"),
            0,
            json!({}),
        )
        .await;
    assert_eq!(
        (response.status(), response.json::<Value>().await.unwrap()),
        expected,
        "POST transcript"
    );
    let status = f
        .request(
            reqwest::Method::POST,
            &format!("{path}/audio/status"),
            0,
            json!({}),
        )
        .await;
    error(status, 404, "not_found").await;
    f.close().await;
}

#[tokio::test]
async fn snapshot_writes_have_their_own_rate_limit() {
    let f = PeerFixture::new("peer-rate-buckets").await;
    let snapshot = "/api/peer/sessions/unknown/rounds/0/snapshot";
    for _ in 0..30 {
        let response = f
            .request(reqwest::Method::PUT, snapshot, 0, json!({}))
            .await;
        assert_ne!(response.status(), 429);
    }
    let limited = f
        .request(reqwest::Method::PUT, snapshot, 0, json!({}))
        .await;
    error(limited, 429, "rate_limited").await;
    for (method, route) in [
        (reqwest::Method::GET, snapshot),
        (
            reqwest::Method::PUT,
            "/api/peer/sessions/unknown/rounds/0/feedback",
        ),
        (reqwest::Method::GET, "/api/peer/sessions/unknown"),
    ] {
        let response = f.request(method.clone(), route, 0, json!({})).await;
        assert_ne!(response.status(), 429, "{method} {route}");
    }
    f.close().await;
}

#[tokio::test]
async fn an_extra_segment_after_start_is_not_found_and_starts_nothing() {
    let f = PeerFixture::new("peer-start-extra").await;
    let path = f.joined().await;
    for member in 0..2 {
        f.post(&format!("{path}/enter"), member, json!({})).await;
    }
    for member in 0..2 {
        let state = f.get(&path, member).await;
        f.post(
            &format!("{path}/ready"),
            member,
            json!({"expectedVersion":state["version"],"ready":true}),
        )
        .await;
    }
    let state = f.get(&path, 0).await;
    let version = state["version"].as_u64().unwrap();
    let rejected = f
        .request(
            reqwest::Method::POST,
            &format!("{path}/start/extra"),
            0,
            json!({"expectedVersion":version,"observerConsent":true}),
        )
        .await;
    error(rejected, 404, "not_found").await;
    assert_eq!(f.get(&path, 0).await["version"].as_u64().unwrap(), version);
    f.close().await;
}

#[tokio::test]
async fn production_peer_writes_accept_only_an_https_origin() {
    let f = PeerFixture::configured("peer-origin-production", |config| {
        config.production = true;
    })
    .await;
    let authority = f.base.trim_start_matches("http://");
    let send = |origin: String| {
        let f = &f;
        async move {
            f.client
                .post(format!("{}/api/peer/sessions", f.base))
                .header("cookie", &f.cookies[0])
                .header("origin", origin)
                .json(&json!({}))
                .send()
                .await
                .unwrap()
        }
    };
    error(
        send(format!("http://{authority}")).await,
        403,
        "origin_mismatch",
    )
    .await;
    let accepted = send(format!("https://{authority}")).await;
    assert_ne!(accepted.status(), 403);
    f.close().await;
}

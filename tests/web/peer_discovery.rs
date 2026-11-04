use super::*;

async fn waiting_room_stub(
    created: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> (String, TestServer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route(
            "/twirp/livekit.RoomService/CreateRoom",
            axum::routing::post(move || async move {
                created.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                axum::Json(json!({}))
            }),
        )
        .route(
            "/twirp/livekit.RoomService/DeleteRoom",
            axum::routing::post(|| async { axum::Json(json!({})) }),
        );
    (
        format!("ws://{address}"),
        spawn_test_server(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        ),
    )
}

async fn fixture(label: &str) -> (PeerFixture, TestServer) {
    let (fixture, room_service, _) = counted_fixture(label, false).await;
    (fixture, room_service)
}

/// A fixture whose room service counts the waiting rooms it is asked to create.
async fn counted_fixture(
    label: &str,
    provision_rooms: bool,
) -> (
    PeerFixture,
    TestServer,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    let created = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (url, room_service) = waiting_room_stub(std::sync::Arc::clone(&created)).await;
    let fixture = PeerFixture::configured(label, move |config| {
        config.provision_rooms = provision_rooms;
        config.pool = primary_pool(&url, "waiting-key", "waiting-secret");
        config.peer_observer =
            codetrial::config::load_peer_observer(&std::collections::BTreeMap::from([(
                "GOOGLE_API_KEY".into(),
                "peer-fixture-key".into(),
            )]))
            .unwrap();
    })
    .await;
    (fixture, room_service, created)
}

async fn nickname(f: &PeerFixture, member: usize, name: &str) -> reqwest::Response {
    f.request(
        reqwest::Method::PUT,
        "/api/peer/profile",
        member,
        json!({"nickname":name}),
    )
    .await
}

#[tokio::test]
async fn discovery_requires_an_english_nickname_and_explicit_opt_in() {
    let (f, room_service) = fixture("peer-discovery-profile").await;
    let before = f.get("/api/peer/profile", 0).await;
    assert!(before["nickname"].is_null());
    for name in ["123", "Bad Name", "a_b", "<script>", "\u{03bb}"] {
        assert_eq!(nickname(&f, 0, name).await.status(), 400, "{name}");
    }
    let saved = nickname(&f, 0, " Lucky7 ").await;
    assert!(
        saved.status().is_success(),
        "{}",
        saved.text().await.unwrap()
    );
    assert_eq!(f.get("/api/peer/profile", 0).await["nickname"], "Lucky7");
    let lobby = f.get("/api/peer/lobby", 1).await;
    assert!(lobby["available"].as_array().unwrap().is_empty());
    assert!(!lobby.to_string().contains("waiting-secret"));
    f.close().await;
    room_service.shutdown().await;
}

#[tokio::test]
async fn reciprocal_pairing_preserves_room_and_needs_both_consents_without_starting_a_clock() {
    let (f, room_service) = fixture("peer-discovery-pair").await;
    for (member, name) in [(0, "Lucky7"), (1, "Grace2"), (2, "Other3")] {
        assert!(nickname(&f, member, name).await.status().is_success());
    }
    let published=f.post("/api/peer/availability",0,json!({"difficulty":"easy","language":"python","roundMinutes":25,"saveTranscript":true,"saveAudio":false})).await;
    let roster = f.get("/api/peer/lobby", 1).await;
    let available = &roster["available"][0];
    assert_eq!(available["nickname"], "Lucky7");
    assert!(available["room"].is_null());
    assert!(available["token"].is_null());
    assert!(available["identity"].is_null());
    let availability = available["availabilityId"].clone();
    let request = f
        .post(
            "/api/peer/pairing-requests",
            1,
            json!({"availabilityId":availability}),
        )
        .await;
    let request_id = request["requestId"].as_str().unwrap();
    let confirm = format!("/api/peer/pairing-requests/{request_id}/confirm");
    let refused = f
        .request(reqwest::Method::POST, &confirm, 0, json!({}))
        .await;
    assert_eq!(refused.status(), 403);
    let first = f.post(&confirm, 0, json!({"observerConsent":true})).await;
    assert!(first["sessionId"].is_null());
    let paired = f.post(&confirm, 1, json!({"observerConsent":true})).await;
    let id = paired["sessionId"].as_str().unwrap();
    let state = f.get(&format!("/api/peer/sessions/{id}"), 0).await;
    assert!(state["currentRound"].is_null());
    assert!(state["hardDeadline"].is_null());
    assert_eq!(state["settings"]["saveTranscript"], true);
    assert_eq!(state["settings"]["saveAudio"], false);
    assert_eq!(state["you"]["observerConsented"], true);
    assert_eq!(state["members"][0]["nickname"], "Lucky7");
    let waiting_claims = claims(published["token"].as_str().unwrap());
    assert_eq!(waiting_claims["sub"], state["members"][0]["identity"]);
    assert_eq!(
        f.request(
            reqwest::Method::GET,
            &format!("/api/peer/sessions/{id}"),
            2,
            Value::Null
        )
        .await
        .status(),
        404
    );
    assert!(
        f.get("/api/peer/lobby", 2).await["available"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.close().await;
    room_service.shutdown().await;
}

#[tokio::test]
async fn unavailable_recording_and_withdrawn_roster_are_not_offered_to_partners() {
    let (f, room_service) = fixture("peer-discovery-withdraw").await;
    assert!(nickname(&f, 0, "Lucky7").await.status().is_success());
    assert!(nickname(&f, 1, "Grace2").await.status().is_success());
    let recording = f
        .request(
            reqwest::Method::POST,
            "/api/peer/availability",
            0,
            json!({"saveAudio":true}),
        )
        .await;
    assert_eq!(recording.status(), 503);
    f.post(
        "/api/peer/availability",
        0,
        json!({"saveTranscript":false,"saveAudio":false}),
    )
    .await;
    let roster = f.get("/api/peer/lobby", 1).await;
    let id = roster["available"][0]["availabilityId"].clone();
    let response = f
        .request(
            reqwest::Method::DELETE,
            "/api/peer/availability",
            0,
            json!({}),
        )
        .await;
    assert!(response.status().is_success());
    let request = f
        .request(
            reqwest::Method::POST,
            "/api/peer/pairing-requests",
            1,
            json!({"availabilityId":id}),
        )
        .await;
    assert_eq!(request.status(), 410);
    assert!(
        f.get("/api/peer/lobby", 1).await["available"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    f.close().await;
    room_service.shutdown().await;
}

#[tokio::test]
async fn confirmed_discovery_pair_keeps_its_room_reservation_until_cancelled() {
    let (f, room_service) = fixture("peer-discovery-reservation").await;
    for (member, name) in [(0, "Host1"), (1, "Partner2"), (2, "Other3")] {
        assert!(nickname(&f, member, name).await.status().is_success());
    }
    let published = f.post("/api/peer/availability", 0, json!({})).await;
    let request = f
        .post(
            "/api/peer/pairing-requests",
            1,
            json!({"availabilityId":published["availabilityId"]}),
        )
        .await;
    let confirm = format!(
        "/api/peer/pairing-requests/{}/confirm",
        request["requestId"].as_str().unwrap()
    );
    f.post(&confirm, 0, json!({"observerConsent":true})).await;
    let paired = f.post(&confirm, 1, json!({"observerConsent":true})).await;
    let base = format!(
        "/api/peer/sessions/{}",
        paired["sessionId"].as_str().unwrap()
    );

    let competitor = f
        .request(
            reqwest::Method::POST,
            "/api/peer/availability",
            2,
            json!({}),
        )
        .await;
    assert_eq!(competitor.status(), 503);
    assert_eq!(
        competitor.json::<Value>().await.unwrap()["code"],
        "at_capacity"
    );
    let private = f.post("/api/peer/sessions", 2, json!({})).await;
    f.post(
        "/api/peer/invites/accept",
        0,
        json!({"token":private["inviteToken"]}),
    )
    .await;
    let private_enter = format!(
        "/api/peer/sessions/{}/enter",
        private["sessionId"].as_str().unwrap()
    );
    let refused = f
        .request(reqwest::Method::POST, &private_enter, 2, json!({}))
        .await;
    assert_eq!(refused.status(), 503);
    assert_eq!(
        refused.json::<Value>().await.unwrap()["code"],
        "at_capacity"
    );

    for member in [0, 1] {
        f.post(&format!("{base}/enter"), member, json!({})).await;
    }
    let state = f.get(&base, 0).await;
    assert!(
        state["members"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["entered"] == true)
    );
    f.post(
        &format!("{base}/cancel"),
        0,
        json!({"expectedVersion":state["version"]}),
    )
    .await;
    f.post("/api/peer/availability", 2, json!({})).await;
    assert!(
        f.request(
            reqwest::Method::DELETE,
            "/api/peer/availability",
            2,
            json!({})
        )
        .await
        .status()
        .is_success()
    );
    f.post(&private_enter, 2, json!({})).await;
    f.close().await;
    room_service.shutdown().await;
}

#[tokio::test]
async fn a_waiting_room_is_created_once_and_only_when_rooms_are_provisioned() {
    use std::sync::atomic::Ordering;
    let (f, room_service, created) = counted_fixture("peer-discovery-provision", true).await;
    assert!(nickname(&f, 0, "Lucky7").await.status().is_success());
    f.post("/api/peer/availability", 0, json!({})).await;
    assert_eq!(created.load(Ordering::SeqCst), 1);
    f.post("/api/peer/availability", 0, json!({})).await;
    assert_eq!(
        created.load(Ordering::SeqCst),
        1,
        "refreshing a live entry must not create the room again"
    );
    f.close().await;
    room_service.shutdown().await;

    let (f, room_service, created) = counted_fixture("peer-discovery-no-provision", false).await;
    assert!(nickname(&f, 0, "Lucky7").await.status().is_success());
    f.post("/api/peer/availability", 0, json!({})).await;
    assert_eq!(created.load(Ordering::SeqCst), 0);
    f.close().await;
    room_service.shutdown().await;
}

#[tokio::test]
async fn heartbeat_extends_a_live_entry_and_is_refused_without_one() {
    let (f, room_service) = fixture("peer-discovery-heartbeat").await;
    assert!(nickname(&f, 0, "Lucky7").await.status().is_success());
    let missing = f
        .request(
            reqwest::Method::POST,
            "/api/peer/availability/heartbeat",
            0,
            json!({}),
        )
        .await;
    error(missing, 410, "availability_unavailable").await;
    let published = f.post("/api/peer/availability", 0, json!({})).await;
    let beat = f
        .post("/api/peer/availability/heartbeat", 0, json!({}))
        .await;
    assert_eq!(beat["available"], true);
    assert!(beat["expiresAt"].as_i64().unwrap() >= published["expiresAt"].as_i64().unwrap());
    f.close().await;
    room_service.shutdown().await;
}

#[tokio::test]
async fn only_a_post_to_a_pairing_request_action_reaches_it() {
    let (f, room_service) = fixture("peer-discovery-actions").await;
    for (member, name) in [(0, "Lucky7"), (1, "Grace2")] {
        assert!(nickname(&f, member, name).await.status().is_success());
    }
    let published = f.post("/api/peer/availability", 0, json!({})).await;
    let request = f
        .post(
            "/api/peer/pairing-requests",
            1,
            json!({"availabilityId":published["availabilityId"]}),
        )
        .await;
    let id = request["requestId"].as_str().unwrap();
    let send = |method: reqwest::Method, path: String| {
        let f = &f;
        async move { f.request(method, &path, 0, json!({})).await }
    };
    for (method, path) in [
        (
            reqwest::Method::POST,
            "/api/peer/pairing-requests/short".to_owned(),
        ),
        (
            reqwest::Method::POST,
            format!("/api/peer/pairing-requests/{id}/decline/extra"),
        ),
        (
            reqwest::Method::POST,
            format!("/api/peer/availability/{id}/decline"),
        ),
        (
            reqwest::Method::GET,
            format!("/api/peer/pairing-requests/{id}/decline"),
        ),
        (
            reqwest::Method::POST,
            format!("/api/peer/pairing-requests/{id}/other"),
        ),
    ] {
        error(send(method, path.clone()).await, 404, "not_found").await;
    }
    let declined = f
        .post(
            &format!("/api/peer/pairing-requests/{id}/decline"),
            0,
            json!({}),
        )
        .await;
    assert_eq!(declined["status"], "declined");
    f.close().await;
    room_service.shutdown().await;
}

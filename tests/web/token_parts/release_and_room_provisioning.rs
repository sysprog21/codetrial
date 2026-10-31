// Part of token.rs: included in place, so every item here shares that file's
// module.

/// The join token carries the start's metadata, so a start with a full
/// grounding packet mints one longer than any `/api/token` request may be. Its
/// release must still be read, or that start waits out the no-show limit with
/// nothing on the page to say so. The euro sign is three bytes, which is what
/// fills the packet's byte budget within its per-item length.
#[tokio::test]
async fn a_start_with_a_full_grounding_packet_can_still_be_released() {
    let dispatcher = std::sync::Arc::<RecordingDispatcher>::default();
    let (base, server) =
        spawn_web_server_with_dispatcher(web_config(), std::sync::Arc::clone(&dispatcher)).await;
    // Numbered, because a packet repeating an item is refused whole.
    let items = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| format!("{index:02}{}", "\u{20ac}".repeat(91)))
            .collect()
    };
    let grounding = json!({
        "consentVersion": 1,
        "requirements": items(8),
        "skills": items(8),
        "anchors": items(6),
    });

    let granted: Value = http_client()
        .post(format!("{base}/api/token"))
        .json(&json!({ "interviewGrounding": grounding }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let token = granted["token"].as_str().unwrap();
    let metadata: Value =
        serde_json::from_str(claims(token)["metadata"].as_str().unwrap()).unwrap();
    assert_eq!(metadata["interviewGrounding"]["anchors"], json!(items(6)));
    assert!(
        token.len() > MAX_BODY_BYTES,
        "the token must be longer than a request body may be, or this proves nothing: {}",
        token.len()
    );

    let room = granted["roomName"].as_str().unwrap();
    assert_eq!(
        release(&base, &json!({"roomName": room, "token": token})).await,
        204
    );
    assert_eq!(dispatcher.releases.lock().unwrap().len(), 1);
    server.shutdown().await;
}

/// Releases have a bucket of their own, so a flood of them leaves the next
/// `/api/token` from the same address alone.
#[tokio::test]
async fn releases_are_rate_limited_apart_from_token_requests() {
    let (base, server) = spawn_web_server_with_guest_dispatcher(web_config()).await;
    for _ in 0..TOKEN_RATE_LIMIT {
        assert_eq!(release(&base, &json!({})).await, 400);
    }
    assert_eq!(release(&base, &json!({})).await, 429);

    let token = http_client()
        .post(format!("{base}/api/token"))
        .send()
        .await
        .unwrap();
    assert_eq!(token.status(), 200);
    server.shutdown().await;
}

/// Records, in order, the CreateRoom bodies a LiveKit stub received and the
/// rooms the dispatcher was asked to staff, so a test can assert which came
/// first.
type RoomEvents = std::sync::Arc<std::sync::Mutex<Vec<String>>>;

struct OrderRecordingDispatcher(RoomEvents);

impl RoomDispatcher for OrderRecordingDispatcher {
    fn supports_guest_admission(&self) -> bool {
        true
    }

    fn ensure_agent(
        &self,
        room_name: &str,
        _provider: &codetrial::config::Provider,
        _job: codetrial::web::AgentJob,
    ) -> Result<(), DispatchRefusal> {
        self.0.lock().unwrap().push(format!("dispatch {room_name}"));
        Ok(())
    }
}

/// A LiveKit RoomService that logs each CreateRoom body into `events` and
/// answers with `status`. Returns the `ws://` URL a provider would be
/// configured with.
async fn spawn_create_room_stub(
    events: RoomEvents,
    status: axum::http::StatusCode,
) -> (String, TestServer) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = axum::Router::new().fallback(move |uri: axum::http::Uri, body: String| {
        let events = std::sync::Arc::clone(&events);
        async move {
            if uri.path() == "/twirp/livekit.RoomService/CreateRoom" {
                events.lock().unwrap().push(format!("create {body}"));
            }
            (status, [("content-type", "application/json")], "{}")
        }
    });
    let server = spawn_test_server(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    );
    (format!("ws://127.0.0.1:{}", addr.port()), server)
}

/// `/api/token` creates the room with the interview's timeouts on the
/// provider that signed the token, and does so before the dispatcher can start
/// an agent that would otherwise create it with LiveKit's 20 second departure
/// default. The body is pinned literally: 100 is the 90 second absence limit
/// plus one 2 second watch tick and an 8 second margin, 330 the agent's
/// candidate wait plus a margin.
#[tokio::test]
async fn token_creates_the_room_with_interview_timeouts_before_dispatch() {
    let events = RoomEvents::default();
    let (provider_url, stub) =
        spawn_create_room_stub(std::sync::Arc::clone(&events), axum::http::StatusCode::OK).await;
    let (mut config, cookie, db_path) = signed_in_web_config("room-create-order");
    config.provision_rooms = true;
    config.pool = primary_pool(&provider_url, "devkey", "devsecret");
    let (base, server) = spawn_web_server_with_dispatcher(
        config,
        std::sync::Arc::new(OrderRecordingDispatcher(std::sync::Arc::clone(&events))),
    )
    .await;

    let response = http_client()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .json(&json!({"problemId":"two-sum","durationMin":45}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let room = body["roomName"].as_str().unwrap().to_string();

    let events = events.lock().unwrap().clone();
    assert_eq!(events.len(), 2, "{events:?}");
    let created: Value =
        serde_json::from_str(events[0].strip_prefix("create ").expect("create first")).unwrap();
    assert_eq!(
        created,
        json!({"name": room, "empty_timeout": 330, "departure_timeout": 100})
    );
    assert_eq!(events[1], format!("dispatch {room}"));

    server.shutdown().await;
    stub.shutdown().await;
    remove_database(db_path).await;
}

/// A LiveKit hiccup on room creation must not become an outage: the token is
/// still minted, the creation is retried once, and the agent is still
/// dispatched after it.
#[tokio::test]
async fn token_still_starts_the_interview_when_room_creation_fails() {
    let events = RoomEvents::default();
    let (provider_url, stub) = spawn_create_room_stub(
        std::sync::Arc::clone(&events),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
    )
    .await;
    let (mut config, cookie, db_path) = signed_in_web_config("room-create-failure");
    config.provision_rooms = true;
    config.pool = primary_pool(&provider_url, "devkey", "devsecret");
    let (base, server) = spawn_web_server_with_dispatcher(
        config,
        std::sync::Arc::new(OrderRecordingDispatcher(std::sync::Arc::clone(&events))),
    )
    .await;

    let response = http_client()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .json(&json!({"problemId":"two-sum","durationMin":45}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let text = response.text().await.unwrap();
    let body: Value = serde_json::from_str(&text).unwrap();
    assert!(body.get("error").is_none(), "{text}");
    assert!(
        body["token"]
            .as_str()
            .is_some_and(|token| !token.is_empty())
    );

    let events = events.lock().unwrap().clone();
    assert_eq!(
        events.iter().filter(|e| e.starts_with("create ")).count(),
        2,
        "one retry: {events:?}"
    );
    assert!(
        events.last().unwrap().starts_with("dispatch "),
        "{events:?}"
    );

    server.shutdown().await;
    stub.shutdown().await;
    remove_database(db_path).await;
}

/// Without the opt-in the handler makes no RoomService call, which is what
/// keeps every other web test off the network.
#[tokio::test]
async fn token_makes_no_room_service_call_unless_provisioning_is_enabled() {
    let events = RoomEvents::default();
    let (provider_url, stub) =
        spawn_create_room_stub(std::sync::Arc::clone(&events), axum::http::StatusCode::OK).await;
    let (mut config, cookie, db_path) = signed_in_web_config("room-create-off");
    config.pool = primary_pool(&provider_url, "devkey", "devsecret");
    let (base, server) = spawn_web_server(config).await;

    let response = http_client()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(events.lock().unwrap().is_empty());

    server.shutdown().await;
    stub.shutdown().await;
    remove_database(db_path).await;
}

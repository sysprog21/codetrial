// Initial bounded generation is distinct from provider attempts and receipts.

#[tokio::test]
async fn g2_bounded_report_observes_only_polled_scoped_invocations() {
    if isolated_checked_with_loopback(
        "g2_bounded_report_observes_only_polled_scoped_invocations",
        true,
        |_| {},
    ) {
        return;
    }
    let config = g2_fixture_config();
    let boot = crate::runtime::bootstrap(&config, "g2-polled-entry", Some("two-sum"), 45);
    let keys = crate::gemini::GeminiKeys::single("g2-fixture-key");
    let refused = std::sync::atomic::AtomicBool::new(false);
    let scope = crate::gemini::tests::ScopedReport::register(boot.room_name, "http://127.0.0.1:9/")
        .unwrap();
    let future = super::super::report::generate_report_bounded(
        &boot,
        "fixture evidence",
        &[],
        (
            crate::agent::ReportRounds {
                behavioral_opened: true,
                follow_ups_released: true,
                candidate_turns: 1,
            },
            false,
        ),
        &keys,
        31,
        &refused,
    );
    tokio::pin!(future);
    assert!(
        scope.entries().is_empty(),
        "constructing a lazy generation does not start it"
    );

    // The refused endpoint keeps the generation retrying, so the first poll
    // leaves it pending and the second one is a real second poll.
    let finished = std::future::poll_fn(|cx| {
        std::task::Poll::Ready(std::future::Future::poll(future.as_mut(), cx).is_ready())
    })
    .await;
    assert!(!finished, "the first poll leaves the generation pending");
    assert_eq!(scope.entries().len(), 1, "actual bounded body entered once");
    let _ = std::future::poll_fn(|cx| {
        std::task::Poll::Ready(std::future::Future::poll(future.as_mut(), cx).is_ready())
    })
    .await;
    assert_eq!(
        scope.entries().len(),
        1,
        "polling the same future does not recount"
    );
    assert_eq!(scope.entries()[0].0, 31);
}

#[tokio::test]
async fn g2_scoped_report_uses_actual_generator_material_and_wire() {
    if isolated_checked_with_loopback(
        "g2_scoped_report_uses_actual_generator_material_and_wire",
        true,
        |_| {},
    ) {
        return;
    }
    let config = g2_fixture_config();
    let boot = crate::runtime::bootstrap(&config, "g2-generator-wire", Some("two-sum"), 45);
    let keys = crate::gemini::GeminiKeys::single("g2-fixture-key");
    let refused = std::sync::atomic::AtomicBool::new(false);
    let fixture = G2ReportFixture::start(boot.room_name).await;
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        super::super::report::generate_report_bounded(
            &boot,
            "unique g2 assessment material",
            &[],
            (
                crate::agent::ReportRounds {
                    behavioral_opened: true,
                    follow_ups_released: true,
                    candidate_turns: 1,
                },
                false,
            ),
            &keys,
            37,
            &refused,
        ),
    )
    .await;
    result
        .expect("registered report route reaches its local HTTP fixture")
        .unwrap()
        .unwrap();
    assert_eq!(fixture.scope.entries().len(), 1);
    let bodies = fixture.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0]
            .to_string()
            .contains("unique g2 assessment material")
    );
    assert_eq!(bodies[0]["generationConfig"]["seed"], 37);
    assert_eq!(
        bodies[0]["generationConfig"]["responseSchema"],
        crate::agent::report_response_schema(true)
    );
}

#[test]
fn g2_repeated_server_end_cannot_create_a_second_finish_intent() {
    let mut state = RuntimeState::default();
    let payload = serde_json::json!({"type":"end_interview", "reason":"candidate_ended"});
    let first = crate::agent::apply_server_event_at(&mut state, TOPIC_CONTROL, &payload, 0.0, 1);
    assert!(first.finish_interview.is_some());
    let deadline = serde_json::json!({"type":"end_interview", "reason":"time_up"});
    let second = crate::agent::apply_server_event_at(&mut state, TOPIC_CONTROL, &deadline, 0.0, 2);
    assert!(
        second.finish_interview.is_none(),
        "deadline re-entry cannot finish an ended assessment twice"
    );
}

fn g2_fixture_config() -> crate::config::AgentConfig {
    load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "g2-fixture-key"),
    ])
    .unwrap()
}

struct G2ReportFixture {
    scope: crate::gemini::tests::ScopedReport,
    bodies: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    server: tokio::task::JoinHandle<()>,
}

impl G2ReportFixture {
    async fn start(scope: &str) -> Self {
        let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = std::sync::Arc::clone(&bodies);
        let answer = crate::gemini::tests::valid_report();
        let app = axum::Router::new().route("/", axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let seen = std::sync::Arc::clone(&seen);
            let answer = answer.clone();
            async move {
                seen.lock().unwrap().push(body);
                axum::Json(serde_json::json!({"candidates":[{"content":{"parts":[{"text":answer.to_string()}]}}]}))
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let scope = crate::gemini::tests::ScopedReport::register(scope, &endpoint).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            scope,
            bodies,
            server,
        }
    }
}

impl Drop for G2ReportFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

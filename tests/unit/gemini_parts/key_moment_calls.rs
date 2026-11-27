// Part of gemini.rs: included in place, so every item here shares that file's
// module.

/// The key-moment notes are asked for against their schema, under their own
/// instruction, and a rate limit on them leaves the key to the report call
/// running beside them.
#[tokio::test]
async fn the_key_moment_notes_request_their_schema_and_spare_the_report_key() {
    let keys = GeminiKeys::single("key-moments-key");
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    let app = axum::Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let seen = Arc::clone(&seen);
            async move {
                seen.lock().unwrap().push(body);
                axum::Json(
                    json!({"candidates":[{"content":{"parts":[{"text":"{\"notes\": []}"}]}}]}),
                )
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    assert_eq!(
        generate_key_moments_at(&keys, &url, "prompt", "test-room")
            .await
            .unwrap(),
        "{\"notes\": []}"
    );
    server.abort();
    let body = bodies.lock().unwrap()[0].clone();
    assert_eq!(
        body["generationConfig"]["responseSchema"],
        crate::agent::key_moments_response_schema()
    );
    assert_eq!(
        body["systemInstruction"]["parts"][0]["text"],
        crate::agent::key_moments_system_instruction()
    );

    let config = live_config(&[("GOOGLE_API_KEYS", "moments-first,moments-second")]);
    let keys = GeminiKeys::from_config(&config);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app = axum::Router::new().route(
        "/",
        axum::routing::post(|| async { axum::http::StatusCode::TOO_MANY_REQUESTS }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    assert!(
        generate_key_moments_at(&keys, &url, "prompt", "test-room")
            .await
            .is_err()
    );
    server.abort();
    assert_eq!(keys.select_report().unwrap(), "moments-first");
}

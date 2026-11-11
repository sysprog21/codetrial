use super::*;

/// The stage an answer's body failure is logged under says what failed: the
/// size cap, a body that would not decode, or the read itself, a reset or a
/// timeout, which is not a decode failure however it was read.
#[tokio::test]
async fn a_body_failure_is_named_for_what_failed() {
    let over: Box<dyn std::error::Error + Send + Sync> =
        io::Error::new(io::ErrorKind::InvalidData, "too long").into();
    assert_eq!(body_stage(over.as_ref()), "body_limit");

    let undecodable = reqwest::Response::from(axum::http::Response::new("not json"))
        .json::<Value>()
        .await
        .unwrap_err();
    assert_eq!(body_stage(&undecodable), "decode");

    let reset: Box<dyn std::error::Error + Send + Sync> = "connection reset".into();
    assert_eq!(body_stage(reset.as_ref()), "body_read");
}

/// The cap is on the body's bytes: one at the cap is read, one past it is not.
#[tokio::test]
async fn the_answer_cap_admits_exactly_its_size() {
    let cap = MAX_RESPONSE_BODY_BYTES;
    // `{"p":"` and `"}` take eight bytes around the padding.
    let answer = |padding: usize| {
        let padding = "x".repeat(padding);
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move || {
                let padding = padding.clone();
                async move { axum::Json(json!({ "p": padding })) }
            }),
        );
        async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let read =
                post_generate_content("k", &url, &json!({}), Duration::from_secs(20), "cap").await;
            server.abort();
            read
        }
    };

    // A stated size, which the constant cannot move with it.
    assert!(answer(3 << 20).await.is_ok(), "a body well under the cap");
    assert!(answer(cap - 8).await.is_ok(), "a body of exactly the cap");
    assert!(answer(cap - 7).await.is_err(), "a body one byte past it");
}

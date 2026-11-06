use super::*;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn peer_audio_understanding_requests_only_text_and_uses_existing_credential_transport() {
    let seen = Arc::new(Mutex::new(None));
    let recorded = Arc::clone(&seen);
    let app=axum::Router::new().fallback(move|headers:axum::http::HeaderMap,axum::Json(body):axum::Json<Value>|{
        let recorded=Arc::clone(&recorded);
        async move {
            assert_eq!(headers["x-goog-api-key"],"observer-test-key");
            *recorded.lock().unwrap()=Some(body);
            axum::Json(json!({"candidates":[{"content":{"parts":[{"text":"{\"text\":\"A technical explanation\"}"}]}}]}))
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/generateContent", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut config = crate::config::load_peer_observer(&std::collections::BTreeMap::from([(
        "GOOGLE_API_KEY".into(),
        "observer-test-key".into(),
    )]))
    .unwrap()
    .unwrap();
    config.endpoint = Some(url);
    let result = generate_peer_content(
        &config,
        "Transcribe only",
        vec![json!({"inlineData":{"mimeType":"audio/wav","data":"UklGRg=="}})],
        "peer-unit",
    )
    .await
    .unwrap();
    assert_eq!(result["text"], "A technical explanation");
    let request = seen.lock().unwrap().take().unwrap();
    assert_eq!(
        request["generationConfig"]["responseModalities"],
        json!(["TEXT"])
    );
    assert!(request["generationConfig"].get("speechConfig").is_none());
    assert!(request.get("tools").is_none());
    assert!(request.get("setup").is_none());
    assert_eq!(
        request["contents"][0]["parts"][0]["inlineData"]["mimeType"],
        "audio/wav"
    );
    server.abort();
}

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
    let mut config = crate::gemini::load_peer_observer(&std::collections::BTreeMap::from([(
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

/// A rate limit on a peer transcription must not cool the report key. Peer
/// rooms share the key list with interviews, and cooled keys leave the final
/// report of a concurrent interview with none to use, as a phase judgment's
/// rate limit is documented not to. Cooldowns are kept per key for the whole
/// process and only where a backup exists, so the keys here are this test's own
/// and there are two of them.
#[tokio::test]
async fn a_rate_limit_on_a_peer_call_leaves_the_report_keys_usable() {
    let app = axum::Router::new().fallback(|| async {
        (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            axum::Json(json!({"error": {"code": 429, "status": "RESOURCE_EXHAUSTED"}})),
        )
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/generateContent", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut config = crate::gemini::load_peer_observer(&std::collections::BTreeMap::from([(
        "GOOGLE_API_KEYS".into(),
        "peer-429-key-a,peer-429-key-b".into(),
    )]))
    .unwrap()
    .unwrap();
    config.endpoint = Some(url);
    for _ in 0..2 {
        let refused =
            generate_peer_content(&config, "Transcribe only", vec![json!({})], "peer-429")
                .await
                .expect_err("the provider refused it");

        // Refused by the rate limit itself, not for some unrelated reason that
        // would leave the keys untouched whatever the call passed.
        let failure = refused.downcast_ref::<crate::gemini::ApiFailure>();
        assert_eq!(
            failure.and_then(|failure| failure.credential),
            Some(crate::gemini::CredentialFailure::Quota),
            "{refused}"
        );
    }
    config
        .keys
        .select_report()
        .expect("a peer rate limit does not cool the report keys");
    server.abort();
}

#[cfg(unix)]
#[test]
fn an_unusable_audio_directory_says_what_is_wrong_with_it() {
    use std::os::unix::fs::PermissionsExt;

    // A name of its own, created exclusively: create_dir fails on anything
    // already there, a stale directory or a symlink included, so the mode
    // change below can only reach the directory this test made.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "codetrial-peer-audio-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = crate::gemini::load_peer_observer(&std::collections::BTreeMap::from([
        ("GOOGLE_API_KEY".into(), "observer-test-key".into()),
        ("CODETRIAL_PEER_AUDIO_DIR".into(), dir.display().to_string()),
    ]))
    .unwrap_err();
    std::fs::remove_dir(&dir).unwrap();
    assert!(error.starts_with("CODETRIAL_PEER_AUDIO_DIR"), "{error}");
    assert!(error.contains("0700"), "{error}");
}

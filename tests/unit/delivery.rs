//! The `tests` module of `src/delivery.rs`, which declares this file by path.
//! Everything here reaches into `src/delivery.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::*;

#[test]
fn dates_are_rfc_3339_in_utc() {
    // The shape Drive wants, and the arithmetic that produces it, checked
    // against dates a leap year and a century get wrong when it is written by
    // hand.
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(rfc3339(1_771_632_000), "2026-02-21T00:00:00Z");
    assert_eq!(
        rfc3339(1_771_632_000 + 24 * 60 * 60),
        "2026-02-22T00:00:00Z"
    );
    // 2000 was a leap year and 1900 was not; the day after 2000-02-28.
    assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    assert_eq!(rfc3339(1_767_225_599), "2025-12-31T23:59:59Z");
}

#[test]
fn object_names_survive_their_slashes() {
    // The JSON API takes the whole object name as one path segment, so a prefix
    // that arrived unescaped would name a different object, or none.
    assert_eq!(
        percent_encode_component("codetrial/abc123.mp4"),
        "codetrial%2Fabc123.mp4"
    );
    assert_eq!(percent_encode_component("a b"), "a%20b");
    assert_eq!(
        percent_encode_component("plain-name_1.mp4"),
        "plain-name_1.mp4"
    );
}

#[test]
fn a_deletion_that_finds_nothing_succeeded() {
    assert!(gone_or_ok(204, "x").is_ok());
    assert!(
        gone_or_ok(404, "x").is_ok(),
        "retention runs more than once"
    );
    assert!(gone_or_ok(403, "x").is_err());
}

/// `bytes=0-n`, where `n` is the last byte the session holds.
fn with_range(range: Option<&str>) -> reqwest::Response {
    let mut builder = axum::http::Response::builder().status(308);
    if let Some(range) = range {
        builder = builder.header(axum::http::header::RANGE, range);
    }
    reqwest::Response::from(builder.body(String::new()).unwrap())
}

/// The offset a `308` moves the upload to. Everything here is one
/// subtraction away from writing a file with a hole in it: the loop trusts
/// this number and never re-sends what it says arrived.
#[test]
fn a_resume_offset_is_believed_only_when_it_can_be_read_exactly() {
    // A session that has nothing yet sends no header at all, which is not an
    // error and must not be read as one.
    assert_eq!(stored_bytes(&with_range(None), 1_000).unwrap(), 0);

    // Inclusive end, so the count is one more than the index. Off by one here
    // re-sends a byte or skips one, and only the second is visible.
    assert_eq!(
        stored_bytes(&with_range(Some("bytes=0-0")), 1_000).unwrap(),
        1
    );
    assert_eq!(
        stored_bytes(&with_range(Some("bytes=0-999")), 1_000).unwrap(),
        1_000
    );
    assert_eq!(
        stored_bytes(&with_range(Some("bytes=0-42 ")), 1_000).unwrap(),
        43,
        "a trailing space is still a readable range"
    );

    // Anything that is not `bytes=0-n` is a header this code cannot act on.
    // Guessing at one moves the offset past bytes that were never stored, so
    // each of these has to be an error and not a zero.
    for range in [
        "bytes=500-999",
        "bytes=0-abc",
        "bytes=0-",
        "0-999",
        "",
        "bytes=0--1",
    ] {
        assert!(
            stored_bytes(&with_range(Some(range)), 1_000).is_err(),
            "{range:?} must not be guessed at"
        );
    }

    // `end + 1` on the largest value a range can name. Wrapping it would report
    // an empty session and restart a finished upload from zero.
    assert!(
        stored_bytes(&with_range(Some(&format!("bytes=0-{}", u64::MAX))), 1_000).is_err(),
        "an offset that overflows must be refused"
    );

    // More than the object has means the session is not the one this transfer
    // opened. Believing it finishes a truncated file.
    assert!(
        stored_bytes(&with_range(Some("bytes=0-1000")), 1_000).is_err(),
        "a session cannot hold more than the object has"
    );
}

#[test]
fn a_service_account_without_a_key_is_refused_at_startup() {
    let now: Arc<dyn Fn() -> i64 + Send + Sync> = Arc::new(|| 0);
    assert!(
        GoogleDelivery::new("{}", "bucket", "drive", Arc::clone(&now)).is_err(),
        "a credential this broken must stop a deployment, not an interview"
    );
    assert!(GoogleDelivery::new("not json", "bucket", "drive", Arc::clone(&now)).is_err());

    // Base64 that decodes and is not a key. Without the `ring` check this was
    // accepted at startup and failed at the first delivery, which is the
    // failure the doc comment above `new` promises not to have.
    let pretend = serde_json::json!({
        "client_email": "codetrial@example.iam.gserviceaccount.com",
        "private_key": "-----BEGIN PRIVATE KEY-----\nbm90IGEga2V5\n-----END PRIVATE KEY-----\n",
    })
    .to_string();
    assert!(GoogleDelivery::new(&pretend, "bucket", "drive", now).is_err());
}

#[test]
fn service_account_validation_agrees_at_configuration_and_startup() {
    for field in ["client_email", "private_key"] {
        for value in [
            serde_json::Value::Null,
            json!(false),
            json!(""),
            json!(" \t\n"),
        ] {
            let mut credentials =
                json!({"client_email": "delivery@example.com", "private_key": "key"});
            credentials[field] = value;
            let credentials = credentials.to_string();
            let expected = format!("the service account has no {field}");
            assert_eq!(
                readable_service_account(&credentials),
                Err(expected.clone())
            );
            assert_eq!(
                GoogleDelivery::new(&credentials, "bucket", "drive", Arc::new(|| 0)).err(),
                Some(expected)
            );
        }
    }
}

/// A delivery whose staged-object reads and Drive calls both go to `base`.
/// Nothing here signs, so the credential only has to exist.
fn delivery_reading_from(base: String) -> GoogleDelivery {
    GoogleDelivery {
        http: crate::http_client_builder().no_proxy().build().unwrap(),
        credentials: ServiceAccount {
            client_email: String::new(),
            private_key: Vec::new(),
            token_uri: String::new(),
        },
        bucket: "staging".to_string(),
        drive_id: "drive".to_string(),
        token: Mutex::new(None),
        now: Arc::new(|| 0),
        storage_api: base.clone(),
        google_api: base,
    }
}

#[tokio::test]
async fn only_an_unauthorized_response_discards_the_cached_token() {
    let delivery = delivery_reading_from(String::new());
    for status in [200, 403, 429, 500, 401] {
        *delivery.token.lock().await = Some(CachedToken {
            value: "cached-token".into(),
            expires_at: 3600,
        });
        let response = reqwest::Response::from(
            axum::http::Response::builder()
                .status(status)
                .body(String::new())
                .unwrap(),
        );
        delivery.note_status(&response).await;
        assert_eq!(delivery.token.lock().await.is_none(), status == 401);
    }
}

/// Serve `respond` on loopback and record each request's path, query and
/// `Range` header, in that order.
async fn storage_stub(
    respond: fn() -> axum::response::Response,
) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let router = axum::Router::new().fallback(
        move |uri: axum::http::Uri, headers: axum::http::HeaderMap| async move {
            let range = headers
                .get(reqwest::header::RANGE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("")
                .to_string();
            log.lock().unwrap().push(format!("{uri} {range}"));
            respond()
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("http://{address}"), seen)
}

/// The size is what the resumable upload declares up front, so a wrong one
/// finishes a truncated file or never finishes at all.
#[tokio::test]
async fn the_staged_size_is_read_from_the_named_object() {
    use axum::response::IntoResponse;
    let (base, seen) = storage_stub(|| axum::Json(json!({ "size": "4096" })).into_response()).await;
    let delivery = delivery_reading_from(base);

    assert_eq!(
        delivery.object_size("codetrial/abc.mp4", "token").await,
        Ok(4096)
    );
    assert_eq!(*seen.lock().unwrap(), ["/b/staging/o/codetrial%2Fabc.mp4 "]);
}

/// A chunk is uploaded as returned, so its bytes have to be the object's.
#[tokio::test]
async fn a_range_returns_the_bytes_the_object_holds() {
    use axum::response::IntoResponse;
    let (base, seen) =
        storage_stub(|| (axum::http::StatusCode::PARTIAL_CONTENT, "hello").into_response()).await;
    let delivery = delivery_reading_from(base);

    assert_eq!(
        delivery
            .object_range("codetrial/abc.mp4", "token", 10, 14)
            .await,
        Ok(b"hello".to_vec())
    );
    assert_eq!(
        *seen.lock().unwrap(),
        ["/b/staging/o/codetrial%2Fabc.mp4?alt=media bytes=10-14"]
    );
}

/// The id found is the file a resumed transfer continues into, so a wrong one
/// writes into a file that is not this recording's, and a missed one uploads
/// a second copy.
#[tokio::test]
async fn the_duplicate_search_returns_the_file_it_found() {
    use axum::response::IntoResponse;
    let (base, seen) =
        storage_stub(|| axum::Json(json!({ "files": [{ "id": "drive-file-7" }] })).into_response())
            .await;
    let delivery = delivery_reading_from(base);
    assert_eq!(
        delivery.existing_file("rec-1", "token").await,
        Ok(Some("drive-file-7".to_string()))
    );
    assert!(seen.lock().unwrap()[0].starts_with("/drive/v3/files?q="));

    let (base, _) = storage_stub(|| axum::Json(json!({ "files": [] })).into_response()).await;
    let delivery = delivery_reading_from(base);
    assert_eq!(delivery.existing_file("rec-1", "token").await, Ok(None));
}

/// The chunks go to the URI the session answered with, so any other one sends
/// the recording nowhere.
#[tokio::test]
async fn an_upload_session_is_the_location_it_answered_with() {
    use axum::response::IntoResponse;
    let (base, seen) = storage_stub(|| {
        (
            [(
                axum::http::header::LOCATION,
                "https://upload.example/session-9",
            )],
            "",
        )
            .into_response()
    })
    .await;
    let delivery = delivery_reading_from(base);
    assert_eq!(
        delivery
            .upload_session("interview.mp4", "rec-1", "token")
            .await,
        Ok("https://upload.example/session-9".to_string())
    );
    assert_eq!(
        *seen.lock().unwrap(),
        ["/upload/drive/v3/files?uploadType=resumable&supportsAllDrives=true "]
    );
}

/// An object that reports no bytes has nothing to upload, and declaring a
/// zero-length session would finish an empty file.
#[tokio::test]
async fn an_empty_staged_object_is_refused() {
    use axum::response::IntoResponse;
    let (base, _) = storage_stub(|| axum::Json(json!({ "size": "0" })).into_response()).await;
    let delivery = delivery_reading_from(base);

    assert!(
        delivery
            .object_size("codetrial/abc.mp4", "token")
            .await
            .is_err()
    );
}

/// A 401 on either read means the remembered token is dead, so the next call
/// has to mint a new one instead of failing the same way until it expires.
#[tokio::test]
async fn a_rejected_token_is_forgotten_by_both_reads() {
    use axum::response::IntoResponse;
    let (base, _) = storage_stub(|| axum::http::StatusCode::UNAUTHORIZED.into_response()).await;
    let delivery = delivery_reading_from(base);
    let remember = |delivery: &GoogleDelivery| {
        *delivery.token.try_lock().unwrap() = Some(CachedToken {
            value: "stale".to_string(),
            expires_at: i64::MAX,
        });
    };

    remember(&delivery);
    assert!(
        delivery
            .object_size("codetrial/abc.mp4", "stale")
            .await
            .is_err()
    );
    assert!(delivery.token.try_lock().unwrap().is_none(), "object_size");

    remember(&delivery);
    assert!(
        delivery
            .object_range("codetrial/abc.mp4", "stale", 0, 4)
            .await
            .is_err()
    );
    assert!(delivery.token.try_lock().unwrap().is_none(), "object_range");
}

/// The clock can cross the token margin after any provider call. Exercise the
/// real transfer and HTTP paths with a loopback OAuth, Storage and Drive stub.
async fn transfer_with_expiry(
    expire_after: &'static str,
    token_fails: bool,
) -> (Result<String, String>, Vec<String>) {
    use axum::body::Bytes;
    use axum::extract::{DefaultBodyLimit, State};
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use std::sync::atomic::{AtomicI64, Ordering};

    struct Stub {
        base: String,
        now: Arc<AtomicI64>,
        expire_after: &'static str,
        token_fails: bool,
        seen: std::sync::Mutex<Vec<String>>,
    }

    async fn answer(
        State(stub): State<Arc<Stub>>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let path = uri.path();
        let bearer = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        stub.seen
            .lock()
            .unwrap()
            .push(format!("{method} {path} {bearer}"));
        if path == "/token" {
            if stub.token_fails {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    axum::Json(json!({"error": "unavailable"})),
                )
                    .into_response();
            }
            return axum::Json(json!({"access_token": "fresh-token", "expires_in": 3600}))
                .into_response();
        }
        if path == "/session" && method == Method::PUT {
            let content_range = headers
                .get(axum::http::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("");
            if content_range == "bytes 0-8388607/8388609"
                && body.len() == UPLOAD_CHUNK_BYTES as usize
                && body.iter().all(|byte| *byte == b'x')
            {
                if stub.expire_after == "chunk" {
                    stub.now.store(121, Ordering::SeqCst);
                }
                return (
                    StatusCode::PERMANENT_REDIRECT,
                    [(axum::http::header::RANGE, "bytes=0-8388607")],
                )
                    .into_response();
            }
            if content_range == "bytes 8388608-8388608/8388609" && body.as_ref() == b"y" {
                return axum::Json(json!({"id": "file-1"})).into_response();
            }
            return StatusCode::BAD_REQUEST.into_response();
        }
        if bearer == "Bearer old-token" && stub.now.load(Ordering::SeqCst) >= 120 {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if path == "/drive/v3/files" {
            if stub.expire_after == "search" {
                stub.now.store(121, Ordering::SeqCst);
            }
            return axum::Json(json!({"files": []})).into_response();
        }
        if path == "/b/staging/o/recording.mp4" {
            if uri.query() == Some("alt=media") {
                let range = headers
                    .get(axum::http::header::RANGE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("");
                let bytes = if range == "bytes=0-8388607" {
                    vec![b'x'; UPLOAD_CHUNK_BYTES as usize]
                } else if range == "bytes=8388608-8388608" {
                    vec![b'y']
                } else {
                    return StatusCode::BAD_REQUEST.into_response();
                };
                return (StatusCode::PARTIAL_CONTENT, bytes).into_response();
            }
            if stub.expire_after == "metadata" {
                stub.now.store(121, Ordering::SeqCst);
            }
            return axum::Json(json!({"size": UPLOAD_CHUNK_BYTES + 1})).into_response();
        }
        if path == "/upload/drive/v3/files" {
            return (
                StatusCode::OK,
                [(
                    axum::http::header::LOCATION,
                    format!("{}/session", stub.base),
                )],
            )
                .into_response();
        }
        StatusCode::NOT_FOUND.into_response()
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let now = Arc::new(AtomicI64::new(0));
    let stub = Arc::new(Stub {
        base: base.clone(),
        now: Arc::clone(&now),
        expire_after,
        token_fails,
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let router = axum::Router::new()
        .fallback(answer)
        .layer(DefaultBodyLimit::disable())
        .with_state(std::sync::Arc::clone(&stub));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let mut delivery = delivery_reading_from(base.clone());
    delivery.credentials.private_key = rsa_key(include_str!("test-only-private-key.pem")).unwrap();
    delivery.credentials.client_email = "delivery@example.com".to_string();
    delivery.credentials.token_uri = format!("{base}/token");
    delivery.now = Arc::new(move || now.load(Ordering::SeqCst));
    *delivery.token.lock().await = Some(CachedToken {
        value: "old-token".to_string(),
        expires_at: 120,
    });

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        delivery.transfer("recording.mp4", "interview.mp4", "rec-1"),
    )
    .await
    .expect("transfer should not hang");
    server.abort();
    let requests = stub.seen.lock().unwrap().clone();
    (result, requests)
}

async fn assert_transfer_refreshes_after(stage: &'static str) {
    let (result, requests) = transfer_with_expiry(stage, false).await;
    assert_eq!(result, Ok("file-1".to_string()), "{stage}: {requests:?}");
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("POST /token "))
            .count(),
        1,
        "{stage}: {requests:?}"
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("POST /upload/drive/v3/files "))
            .count(),
        1,
        "{stage}: {requests:?}"
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("PUT /session "))
            .count(),
        2,
        "{stage}: {requests:?}"
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("GET /b/staging/o/recording.mp4 "))
            .count(),
        3,
        "metadata and exactly two ranges: {stage}: {requests:?}"
    );
    assert!(
        requests
            .iter()
            .any(|request| request == "GET /b/staging/o/recording.mp4 Bearer fresh-token"),
        "{stage}: {requests:?}"
    );
}

#[tokio::test]
async fn a_transfer_refreshes_the_token_between_chunks() {
    assert_transfer_refreshes_after("chunk").await;
}

#[tokio::test]
async fn a_transfer_refreshes_after_duplicate_search() {
    assert_transfer_refreshes_after("search").await;
}

#[tokio::test]
async fn a_transfer_refreshes_after_metadata() {
    assert_transfer_refreshes_after("metadata").await;
}

#[tokio::test]
async fn a_transfer_reuses_a_valid_cached_token() {
    let (result, requests) = transfer_with_expiry("", false).await;
    assert_eq!(result, Ok("file-1".to_string()), "{requests:?}");
    assert!(
        requests
            .iter()
            .all(|request| !request.starts_with("POST /token "))
    );
    assert_eq!(
        requests.iter().filter(|request| request.starts_with("GET /b/staging/o/recording.mp4 Bearer old-token")).count(),
        3
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("PUT /session "))
            .count(),
        2
    );
}

#[tokio::test]
async fn a_failed_refresh_stops_before_the_next_range() {
    let (result, requests) = transfer_with_expiry("chunk", true).await;
    assert!(
        result.unwrap_err().contains("token request was refused"),
        "{requests:?}"
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("POST /token "))
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("GET /b/staging/o/recording.mp4 "))
            .count(),
        2,
        "metadata and the first range only: {requests:?}"
    );
}

/// The margin is strict: 61 seconds left is reusable, exactly 60 is not.
#[tokio::test]
async fn the_token_refreshes_at_the_sixty_second_margin() {
    use axum::response::IntoResponse;
    use std::sync::atomic::{AtomicI64, Ordering};

    let (base, seen) = storage_stub(|| {
        axum::Json(json!({"access_token": "fresh-token", "expires_in": 3600})).into_response()
    })
    .await;
    let mut delivery = delivery_reading_from(base.clone());
    delivery.credentials.private_key = rsa_key(include_str!("test-only-private-key.pem")).unwrap();
    delivery.credentials.client_email = "delivery@example.com".to_string();
    delivery.credentials.token_uri = format!("{base}/token");
    let clock = Arc::new(AtomicI64::new(59));
    delivery.now = Arc::new({
        let clock = std::sync::Arc::clone(&clock);
        move || clock.load(Ordering::SeqCst)
    });
    *delivery.token.lock().await = Some(CachedToken {
        value: "old-token".to_string(),
        expires_at: 120,
    });

    assert_eq!(delivery.access_token().await, Ok("old-token".to_string()));
    assert!(seen.lock().unwrap().is_empty());
    clock.store(60, Ordering::SeqCst);
    assert_eq!(delivery.access_token().await, Ok("fresh-token".to_string()));
    assert_eq!(seen.lock().unwrap().len(), 1);
    clock.store(61, Ordering::SeqCst);
    assert_eq!(delivery.access_token().await, Ok("fresh-token".to_string()));
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[test]
fn a_key_that_is_not_base64_is_refused_without_quoting_it() {
    // The decode error names the offending byte, which is part of the key.
    let error = rsa_key("-----BEGIN PRIVATE KEY-----\nSECRET$KEY\n-----END PRIVATE KEY-----\n")
        .expect_err("a body that is not base64 is not a key");
    assert_eq!(error, "the private key is not base64");
}

/// The upload session's address is a capability: whoever holds it can write to
/// the upload. A failed status query reaches the audit log inside an error
/// string, and reqwest appends the address it was sending to, so the address
/// has to be left out of it.
#[tokio::test]
async fn a_failed_status_query_does_not_name_the_session_address() {
    let delivery = delivery_reading_from("http://127.0.0.1:9".to_string());
    let error = delivery
        .session_progress("http://127.0.0.1:9/upload?upload_id=SECRET-UPLOAD-ID", 10)
        .await
        .unwrap_err();
    assert!(!error.contains("SECRET-UPLOAD-ID"), "{error}");
    assert!(error.contains("could not be queried"), "{error}");
}

/// What a status query on an upload session can mean, one case each: the bytes
/// it holds, a finished file whose last acknowledgement was lost, a service
/// that is only unwell, and a session that is gone.
#[tokio::test]
async fn a_session_status_says_progress_finished_unwell_or_gone() {
    use axum::response::IntoResponse;
    let answer = |status: u16, range: Option<&'static str>, body: &'static str| async move {
        let router = axum::Router::new().fallback(move || async move {
            let mut response =
                (axum::http::StatusCode::from_u16(status).unwrap(), body).into_response();
            if let Some(range) = range {
                response.headers_mut().insert(
                    axum::http::header::RANGE,
                    axum::http::HeaderValue::from_static(range),
                );
            }
            response
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let delivery = delivery_reading_from(base.clone());
        delivery
            .session_progress(&format!("{base}/session"), 100)
            .await
    };
    assert_eq!(
        answer(308, Some("bytes=0-49"), "").await,
        Ok(SessionState::Holds(50))
    );
    assert_eq!(
        answer(200, None, r#"{"id":"file-9"}"#).await,
        Ok(SessionState::Finished("file-9".to_string())),
        "the last chunk landed and only its answer was lost"
    );
    assert_eq!(
        answer(201, None, "{}").await,
        Err("the upload answer carried no file id".to_string())
    );
    let invalid = answer(200, None, "not json").await.unwrap_err();
    assert!(invalid.starts_with("the upload answer was not JSON: "));
    for status in RETRYABLE_STATUSES {
        assert_eq!(
            answer(status, None, "").await,
            Ok(SessionState::Unwell),
            "{status}"
        );
    }
    assert_eq!(answer(404, None, "").await, Ok(SessionState::Gone));
}

/// A transfer whose one chunk is answered `503`, with the session answering
/// `query` when it is asked what it holds. Returns the result and how many
/// times the chunk was sent.
async fn transfer_whose_chunk_fails(
    query: u16,
    query_body: &'static str,
) -> (Result<String, String>, usize) {
    transfer_whose_chunk_is_answered(503, "", query, query_body).await
}

/// A transfer whose one chunk is answered `chunk` with `chunk_body`, with the
/// session answering `query` when it is asked what it holds. Returns the result
/// and how many times the chunk was sent.
async fn transfer_whose_chunk_is_answered(
    chunk: u16,
    chunk_body: &'static str,
    query: u16,
    query_body: &'static str,
) -> (Result<String, String>, usize) {
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Stub {
        base: String,
        chunk: u16,
        chunk_body: &'static str,
        query: u16,
        query_body: &'static str,
        chunks: AtomicUsize,
    }

    async fn answer(
        State(stub): State<Arc<Stub>>,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        _body: Bytes,
    ) -> Response {
        let range = headers
            .get(axum::http::header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        match uri.path() {
            "/session" if method == Method::PUT && range == "bytes */3" => (
                StatusCode::from_u16(stub.query).unwrap(),
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                stub.query_body,
            )
                .into_response(),
            "/session" if method == Method::PUT => {
                stub.chunks.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::from_u16(stub.chunk).unwrap(),
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    stub.chunk_body,
                )
                    .into_response()
            }
            "/drive/v3/files" => axum::Json(json!({"files": []})).into_response(),
            "/b/staging/o/recording.mp4" if uri.query() == Some("alt=media") => {
                (StatusCode::PARTIAL_CONTENT, vec![b'x'; 3]).into_response()
            }
            "/b/staging/o/recording.mp4" => axum::Json(json!({"size": 3})).into_response(),
            "/upload/drive/v3/files" => (
                StatusCode::OK,
                [(
                    axum::http::header::LOCATION,
                    format!("{}/session", stub.base),
                )],
            )
                .into_response(),
            _ => StatusCode::NOT_FOUND.into_response(),
        }
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let stub = Arc::new(Stub {
        base: base.clone(),
        chunk,
        chunk_body,
        query,
        query_body,
        chunks: AtomicUsize::new(0),
    });
    let router = axum::Router::new()
        .fallback(answer)
        .with_state(Arc::clone(&stub));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let delivery = delivery_reading_from(base);
    *delivery.token.lock().await = Some(CachedToken {
        value: "token".to_string(),
        expires_at: 100_000,
    });
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        delivery.transfer("recording.mp4", "interview.mp4", "rec-1"),
    )
    .await
    .expect("transfer should not hang");
    server.abort();
    (result, stub.chunks.load(Ordering::SeqCst))
}

/// A chunk whose answer was lost, and then the session's own account of itself.
///
/// A session that says the file is finished is believed, and the chunk is not
/// sent again; one that is unwell is asked again on the budget it already
/// spent, and gives up when it runs out; one that is gone ends the attempt.
#[tokio::test]
async fn a_failed_chunk_is_settled_by_what_the_session_says() {
    let (result, chunks) = transfer_whose_chunk_fails(200, r#"{"id":"file-9"}"#).await;
    assert_eq!(result, Ok("file-9".to_string()));
    assert_eq!(chunks, 1, "a finished upload is not sent again");

    let (result, chunks) = transfer_whose_chunk_fails(503, "{}").await;
    assert_eq!(result, Err("the upload ran out of retries".to_string()));
    assert_eq!(
        chunks,
        UPLOAD_RETRY_BUDGET as usize + 1,
        "an unwell session spends the budget and no more"
    );

    let (result, chunks) = transfer_whose_chunk_fails(404, "{}").await;
    assert_eq!(
        result,
        Err("the upload session is no longer open".to_string())
    );
    assert_eq!(chunks, 1);
}

/// The answer to the last chunk names the file, so one that does not is a
/// failed upload and not an empty file id.
#[tokio::test]
async fn a_finished_chunk_answer_without_a_file_id_fails_the_transfer() {
    let (result, chunks) = transfer_whose_chunk_is_answered(201, "{}", 404, "{}").await;
    assert_eq!(
        result,
        Err("the upload answer carried no file id".to_string())
    );
    assert_eq!(chunks, 1);

    let (result, _) = transfer_whose_chunk_is_answered(200, "not json", 404, "{}").await;
    assert!(
        result
            .unwrap_err()
            .starts_with("the upload answer was not JSON: ")
    );
}

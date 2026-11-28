//! The `tests` module of `src/web/update.rs`, which declares this file by path.
//! Everything here reaches into `src/web/update.rs` through `super`, so it is a
//! unit test and not an integration test: private items are in scope.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const RELEASE_COMMIT: &str = "abc1234def5678abc1234def5678abc1234def56";
const OTHER_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// A GitHub that answers the release lookup with `status` and `body`, and
/// counts how often it was asked. Bound to the path `fetch_latest_commit` asks
/// for, so a request for any other path is a 404 and reads as a failed check
/// rather than passing by accident.
async fn github_stub(
    status: axum::http::StatusCode,
    body: Value,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asked);
    let app = axum::Router::new().route(
        RELEASE_PATH,
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let counted = Arc::clone(&counted);
            let body = body.clone();
            async move {
                counted.fetch_add(1, Ordering::Relaxed);
                // GitHub refuses a request without one.
                if headers.get(header::USER_AGENT).is_none() {
                    return (axum::http::StatusCode::FORBIDDEN, String::new());
                }
                (status, body.to_string())
            }
        }),
    );
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://127.0.0.1:{port}"), asked, server)
}

async fn released(commit: &str) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    github_stub(
        axum::http::StatusCode::OK,
        json!({ "tag_name": "latest", "target_commitish": commit }),
    )
    .await
}

#[tokio::test]
async fn a_build_of_the_released_commit_is_offered_nothing() {
    let (base, _, server) = released(RELEASE_COMMIT).await;
    let latest = LatestRelease::default().commit(&base).await;
    assert_eq!(latest.as_deref(), Some(RELEASE_COMMIT));
    assert_eq!(
        answer(&RELEASE_COMMIT[..7], latest.as_deref()),
        json!({ "enabled": true, "available": false })
    );
    server.abort();
}

#[tokio::test]
async fn a_build_of_another_commit_is_offered_the_release() {
    let (base, _, server) = released(OTHER_COMMIT).await;
    let latest = LatestRelease::default().commit(&base).await;
    assert_eq!(
        answer(&RELEASE_COMMIT[..7], latest.as_deref()),
        json!({
            "enabled": true,
            "available": true,
            "current": &RELEASE_COMMIT[..7],
            "latest": &OTHER_COMMIT[..7],
            "url": RELEASE_PAGE,
        })
    );
    server.abort();
}

/// GitHub names the commit in full and the build records seven characters, so
/// the comparison is a prefix one. Read back the other way, every build would
/// be offered the release it already is.
#[test]
fn the_comparison_reads_the_short_build_commit_against_the_full_one() {
    assert_eq!(answer("abc1234", Some(RELEASE_COMMIT))["available"], false);
    assert_eq!(answer("abc1235", Some(RELEASE_COMMIT))["available"], true);
}

/// Every way the check can fail answers "no update", so the lobby shows
/// nothing: an error status (a rate limit is a 403 or a 429), the 404 while
/// `latest` is republished, a body that names no full commit, and a server
/// that is not there at all.
#[tokio::test]
async fn a_failed_check_offers_nothing() {
    let failures = [
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({})),
        (
            axum::http::StatusCode::FORBIDDEN,
            json!({ "message": "API rate limit exceeded" }),
        ),
        (axum::http::StatusCode::TOO_MANY_REQUESTS, json!({})),
        (
            axum::http::StatusCode::NOT_FOUND,
            json!({ "message": "Not Found" }),
        ),
        (axum::http::StatusCode::OK, json!({ "tag_name": "latest" })),
        (
            axum::http::StatusCode::OK,
            json!({ "target_commitish": "main" }),
        ),
        (
            axum::http::StatusCode::OK,
            json!({ "target_commitish": &OTHER_COMMIT[..7] }),
        ),
        (
            axum::http::StatusCode::OK,
            json!({ "target_commitish": "g".repeat(40) }),
        ),
        (axum::http::StatusCode::OK, json!(["not", "a", "release"])),
    ];
    for (status, body) in failures {
        let (base, asked, server) = github_stub(status, body.clone()).await;
        let latest = LatestRelease::default().commit(&base).await;
        assert_eq!(latest, None, "{status} {body}");
        assert_eq!(asked.load(Ordering::Relaxed), 1, "{status} {body}");
        assert_eq!(
            answer(&RELEASE_COMMIT[..7], latest.as_deref()),
            json!({ "enabled": true, "available": false }),
            "{status} {body}"
        );
        server.abort();
    }

    // Nothing listening: bound, then dropped, so the port refuses.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    assert_eq!(LatestRelease::default().commit(&base).await, None);
}

/// One answer serves every tab until it goes stale, failures included, so a
/// row of open lobbies and a reload do not each cost a GitHub request.
#[tokio::test]
async fn one_answer_serves_every_request_until_it_is_stale() {
    let (base, asked, server) = released(OTHER_COMMIT).await;
    let latest = LatestRelease::default();
    let (first, second) = tokio::join!(latest.commit(&base), latest.commit(&base));
    assert_eq!(first.as_deref(), Some(OTHER_COMMIT));
    assert_eq!(second.as_deref(), Some(OTHER_COMMIT));
    assert_eq!(latest.commit(&base).await.as_deref(), Some(OTHER_COMMIT));
    assert_eq!(asked.load(Ordering::Relaxed), 1);

    // Asked again just inside ten minutes and then at ten. Written out rather
    // than read from `ANSWER_TTL`, so the test pins the ten minutes
    // docs/install.md promises and not whatever the constant says.
    let ten_minutes = Duration::from_mins(10);
    let asked_at = latest.answer.lock().await.as_ref().unwrap().0;
    let inside = asked_at + ten_minutes - Duration::from_secs(1);
    assert_eq!(
        latest.commit_at(&base, inside).await.as_deref(),
        Some(OTHER_COMMIT)
    );
    assert_eq!(asked.load(Ordering::Relaxed), 1);
    let past = asked_at + ten_minutes;
    assert_eq!(
        latest.commit_at(&base, past).await.as_deref(),
        Some(OTHER_COMMIT)
    );
    assert_eq!(asked.load(Ordering::Relaxed), 2);
    server.abort();

    let (failing, asked, server) =
        github_stub(axum::http::StatusCode::INTERNAL_SERVER_ERROR, json!({})).await;
    let latest = LatestRelease::default();
    assert_eq!(latest.commit(&failing).await, None);
    assert_eq!(latest.commit(&failing).await, None);
    assert_eq!(asked.load(Ordering::Relaxed), 1);
    server.abort();
}

/// A lobby that reloads or closes while GitHub is still answering drops the
/// request it made. The answer is kept anyway, or every such reload on a slow
/// network would cost another GitHub request.
#[tokio::test]
async fn a_request_dropped_mid_lookup_still_keeps_the_answer() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let asked = Arc::new(AtomicUsize::new(0));

    // The stub says when the request has arrived and answers only when told, so
    // the caller is dropped mid-lookup however loaded the machine is.
    let arrived = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let app = axum::Router::new().route(
        RELEASE_PATH,
        axum::routing::get({
            let (asked, arrived, release) = (
                Arc::clone(&asked),
                Arc::clone(&arrived),
                Arc::clone(&release),
            );
            move || async move {
                asked.fetch_add(1, Ordering::Relaxed);
                arrived.notify_one();
                release.notified().await;
                json!({ "target_commitish": OTHER_COMMIT }).to_string()
            }
        }),
    );
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let latest = LatestRelease::default();
    tokio::select! {
        _ = latest.commit(&base) => panic!("the lookup finished before it was dropped"),
        () = arrived.notified() => {}
    }
    release.notify_one();

    assert_eq!(latest.commit(&base).await.as_deref(), Some(OTHER_COMMIT));
    assert_eq!(asked.load(Ordering::Relaxed), 1);
    server.abort();
}

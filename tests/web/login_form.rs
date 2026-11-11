//! What the login routes accept, and how often, from a page that is not ours.

use super::*;

/// The login body is JSON, and the page says so. A cross-site form cannot set
/// that content type without a preflight, but it can post a body that merely
/// parses as JSON, so the route has to ask for the type, or a third-party page
/// could swap a visitor's session for one it minted.
#[tokio::test]
async fn a_login_post_that_is_not_declared_json_is_refused() {
    let (base, server, path, client) = account_server("login-form").await;
    let body = r#"{"login":"octocat","a":"="}"#;
    for content_type in ["text/plain", "application/x-www-form-urlencoded"] {
        let response = client
            .post(format!("{base}/api/login"))
            .header("content-type", content_type)
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 415, "{content_type}");
        assert!(
            response.headers().get("set-cookie").is_none(),
            "{content_type} minted a session"
        );
    }
    // What the page sends, with a charset as some clients add one.
    let accepted = client
        .post(format!("{base}/api/login"))
        .header("content-type", "Application/JSON; charset=utf-8")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status(), 200);
    server.shutdown().await;
    remove_database(&path).await;
}

/// The callback spends the server's GitHub OAuth quota with its client secret,
/// and the state cookie stays good for ten minutes, so one that was kept can be
/// replayed. The callback counts against the same per-address bucket as the
/// login post, and the request after the allowance is refused before any call.
#[tokio::test]
async fn the_callback_is_charged_to_the_login_bucket() {
    let (base, server, path, client) = account_server("callback-limit").await;
    for attempt in 0..30 {
        let response = client
            .get(format!("{base}/api/callback?code=x&state=y"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "attempt {attempt}");
    }
    let refused = client
        .get(format!("{base}/api/callback?code=x&state=y"))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 429);
    server.shutdown().await;
    remove_database(&path).await;
}

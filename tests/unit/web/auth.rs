use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn serve(router: axum::Router) -> (GitHubLoginConfig, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (
        GitHubLoginConfig {
            oauth: None,
            session_secret: "test".into(),
            db_path: Default::default(),
            oauth_base_url: base.clone(),
            api_base_url: base,
        },
        task,
    )
}

async fn github_stub(
    status: StatusCode,
    body: Value,
) -> (GitHubLoginConfig, tokio::task::JoinHandle<()>) {
    serve(axum::Router::new().fallback(move || {
        let body = body.clone();
        async move { (status, axum::Json(body)) }
    }))
    .await
}

fn oauth() -> GitHubOauth {
    GitHubOauth {
        client_id: "client".into(),
        client_secret: "secret".into(),
    }
}

#[tokio::test]
async fn github_error_bodies_cannot_supply_credentials_or_identity() {
    // The redirects carry a body that would pass on its own, so only the status
    // check can refuse them; the dedicated redirect tests below send none.
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::TEMPORARY_REDIRECT,
        StatusCode::PERMANENT_REDIRECT,
    ] {
        let (config, task) = github_stub(
            status,
            json!({
                "access_token": "untrusted", "id": 303, "login": "octocat"
            }),
        )
        .await;
        assert!(
            github_access_token(&config, &oauth(), "code")
                .await
                .is_err()
        );
        assert!(github_profile(&config, "token", false).await.is_err());
        task.abort();
    }
}

#[tokio::test]
async fn github_email_errors_are_not_read_as_no_address() {
    for status in [
        StatusCode::FORBIDDEN,
        StatusCode::TEMPORARY_REDIRECT,
        StatusCode::PERMANENT_REDIRECT,
    ] {
        let (config, task) = github_stub(
            status,
            json!([{"email": "a@example.com", "primary": true, "verified": true}]),
        )
        .await;
        assert!(primary_verified_email(&config, "token").await.is_err());
        task.abort();
    }
}

#[tokio::test]
async fn github_profiles_require_positive_ids_and_logins() {
    let too_long = "a".repeat(GITHUB_LOGIN_LIMIT + 1);
    for (id, login) in [(0, "octocat"), (-1, "octocat"), (303, ""), (303, &too_long)] {
        let (config, task) = github_stub(StatusCode::OK, json!({"id": id, "login": login})).await;
        assert!(github_profile(&config, "token", false).await.is_err());
        task.abort();
    }
    // Legacy trailing dashes and managed-user underscores are real logins.
    for login in ["octocat", "ra--", "mona-cat_octo"] {
        let (config, task) = github_stub(StatusCode::OK, json!({"id": 303, "login": login})).await;
        let profile = github_profile(&config, "token", false).await.unwrap();
        assert_eq!(profile.github_id, 303);
        assert_eq!(profile.login, login);
        task.abort();
    }
}

#[tokio::test]
async fn oauth_redirect_cannot_replay_client_secret() {
    let forwarded = Arc::new(AtomicUsize::new(0));
    let observed = std::sync::Arc::clone(&forwarded);
    let router = axum::Router::new()
        .route(
            "/login/oauth/access_token",
            axum::routing::post(|| async {
                (
                    StatusCode::TEMPORARY_REDIRECT,
                    [(header::LOCATION, "/capture")],
                )
            }),
        )
        .route(
            "/capture",
            axum::routing::post(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                async { axum::Json(json!({"access_token": "leaked"})) }
            }),
        );
    let (config, task) = serve(router).await;
    assert!(
        github_access_token(&config, &oauth(), "code")
            .await
            .is_err()
    );
    assert_eq!(forwarded.load(Ordering::SeqCst), 0);
    task.abort();
}

#[tokio::test]
async fn api_redirects_cannot_answer_for_github() {
    let forwarded = Arc::new(AtomicUsize::new(0));
    let observed = std::sync::Arc::clone(&forwarded);
    let redirect = || async {
        (
            StatusCode::TEMPORARY_REDIRECT,
            [(header::LOCATION, "/capture")],
        )
    };
    let router = axum::Router::new()
        .route("/user", axum::routing::get(redirect))
        .route("/user/emails", axum::routing::get(redirect))
        .route(
            "/capture",
            axum::routing::get(move || {
                observed.fetch_add(1, Ordering::SeqCst);
                async { axum::Json(json!({"id": 303, "login": "octocat"})) }
            }),
        );
    let (config, task) = serve(router).await;
    assert!(github_profile(&config, "token", false).await.is_err());
    assert!(primary_verified_email(&config, "token").await.is_err());
    assert_eq!(forwarded.load(Ordering::SeqCst), 0);
    task.abort();
}

#[test]
fn a_state_is_honoured_only_within_its_ttl() {
    let now = 1_000_000;
    assert_eq!(
        unexpired_state(&format!("{now}:token"), now).as_deref(),
        Some("token")
    );
    let edge = now - OAUTH_STATE_TTL_SECONDS;
    assert!(unexpired_state(&format!("{edge}:token"), now).is_some());
    for payload in [
        format!("{}:token", edge - 1),
        // Issued in the future is not a clock skew worth trusting.
        format!("{}:token", now + 1),
        "token".to_string(),
        "soon:token".to_string(),
    ] {
        assert!(unexpired_state(&payload, now).is_none(), "{payload}");
    }
}

#[test]
fn a_cookie_signature_verifies_only_under_its_own_name() {
    let signed = signed_cookie_value(SESSION_COOKIE, "id", "secret");
    assert_eq!(
        verified_cookie_value(SESSION_COOKIE, &signed, "secret").as_deref(),
        Some("id")
    );
    assert!(verified_cookie_value(OAUTH_STATE_COOKIE, &signed, "secret").is_none());
}

#[tokio::test]
async fn github_bodies_are_bounded() {
    let padding = "a".repeat(GITHUB_BODY_LIMIT);
    let (config, task) = github_stub(
        StatusCode::OK,
        json!({"id": 303, "login": "octocat", "bio": padding}),
    )
    .await;
    assert!(github_profile(&config, "token", false).await.is_err());
    task.abort();
}

/// Exactly the limit, delivered in pieces, is still a body: the cap counts
/// bytes across chunks rather than refusing at the boundary or judging one
/// chunk against another.
#[tokio::test]
async fn a_body_at_the_limit_arrives_whole_across_chunks() {
    let prefix = r#"{"id":303,"login":"octocat","bio":""#;
    let suffix = r#""}"#;
    let body = format!(
        "{prefix}{}{suffix}",
        "a".repeat(GITHUB_BODY_LIMIT - prefix.len() - suffix.len())
    );
    assert_eq!(body.len(), GITHUB_BODY_LIMIT);
    let chunks = body
        .as_bytes()
        .chunks(64 * 1024)
        .map(<[u8]>::to_vec)
        .collect::<Vec<_>>();
    let (config, task) = serve(axum::Router::new().fallback(move || {
        let chunks = chunks.clone().into_iter().map(Ok::<_, std::io::Error>);
        async move { Body::from_stream(futures_util::stream::iter(chunks)) }
    }))
    .await;
    let profile = github_profile(&config, "token", false).await.unwrap();
    assert_eq!(profile.github_id, 303);
    task.abort();
}

#[tokio::test]
async fn a_recording_profile_carries_the_verified_email() {
    let router = axum::Router::new()
        .route(
            "/user",
            axum::routing::get(|| async { axum::Json(json!({"id": 303, "login": "octocat"})) }),
        )
        .route(
            "/user/emails",
            axum::routing::get(|| async {
                axum::Json(json!([
                    {"email": "old@example.com", "primary": false, "verified": true},
                    {"email": "me@example.com", "primary": true, "verified": true}
                ]))
            }),
        );
    let (config, task) = serve(router).await;
    let profile = github_profile(&config, "token", true).await.unwrap();
    assert_eq!(profile.verified_email.as_deref(), Some("me@example.com"));
    task.abort();
}

#[tokio::test]
async fn a_refused_code_names_githubs_reason() {
    let (config, task) = github_stub(
        StatusCode::OK,
        json!({
            "error": "bad_verification_code",
            "error_description": "The code passed is incorrect or expired."
        }),
    )
    .await;
    let error = github_access_token(&config, &oauth(), "code")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("bad_verification_code"), "{error}");
    assert!(!error.contains("secret"), "{error}");
    task.abort();
}

//! The headers every response carries, and what the Content-Security-Policy
//! names.
//!
//! Split out of `tests/web.rs`, which is still the test target: cargo
//! discovers only `tests/*.rs`, so this compiles as a module of that one
//! binary rather than relinking the crate for a file of its own.

use super::*;

/// HTTPS is not optional in production, and the browser is told so.
///
/// A year, this host only. `includeSubDomains` would be a promise on behalf of
/// names this process has never seen -- the recording template origin among
/// them -- and `preload` is a submission to a list baked into browser binaries
/// that takes months to leave, so the value is pinned here rather than left to
/// grow a directive nobody meant to commit to.
#[tokio::test]
async fn production_promises_the_browser_it_will_stay_on_https() {
    let (base, server) = spawn_web_server(WebServerConfig {
        web_dir: Path::new("web").to_path_buf(),
        pool: primary_pool("wss://example.livekit.cloud:443", "devkey", "devsecret"),
        probe_provider_quota: false,
        production: true,
        ..web_config()
    })
    .await;

    let home = http_client().get(&base).send().await.unwrap();
    assert_eq!(
        home.headers().get("strict-transport-security").unwrap(),
        "max-age=31536000"
    );

    server.shutdown().await;
}

/// The page refuses `eval`; the one worker that cannot is served its own
/// policy that allows it.
///
/// A worker fetched over http(s) takes its policy from its own response rather
/// than inheriting the document's, which is the whole reason MediaPipe's
/// emscripten glue can keep building functions from text without the interview
/// page being allowed to. Both halves are asserted here: granting eval to the
/// worker is only worth anything while the document still refuses it.
#[tokio::test]
async fn only_the_face_worker_may_evaluate_a_string() {
    let (base, server) = spawn_web_server(WebServerConfig {
        web_dir: Path::new("web").to_path_buf(),
        pool: primary_pool("wss://example.livekit.cloud:443", "devkey", "devsecret"),
        probe_provider_quota: false,
        ..web_config()
    })
    .await;

    let page = http_client().get(&base).send().await.unwrap();
    let page_policy = page
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(!page_policy.contains("'unsafe-eval'"), "{page_policy}");
    assert!(page_policy.contains("'wasm-unsafe-eval'"), "{page_policy}");

    let worker = http_client()
        .get(format!("{base}/face-worker.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(worker.status(), reqwest::StatusCode::OK);
    let worker_policy = worker
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(worker_policy.contains("'unsafe-eval'"), "{worker_policy}");

    server.shutdown().await;
}

#[tokio::test]
async fn responses_carry_baseline_security_headers() {
    let (base, server) = spawn_web_server(WebServerConfig {
        web_dir: Path::new("web").to_path_buf(),
        pool: primary_pool("wss://example.livekit.cloud:443", "devkey", "devsecret"),
        probe_provider_quota: false,
        ..web_config()
    })
    .await;

    let home = http_client().get(&base).send().await.unwrap();

    assert_eq!(
        home.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(
        home.headers().get("referrer-policy").unwrap(),
        "same-origin"
    );

    // Asserted rather than trusted to stay: a header that grants nothing
    // visible is the kind that disappears in a refactor with nobody noticing,
    // because no page stops working when it does.
    assert_eq!(
        home.headers().get("permissions-policy").unwrap(),
        "geolocation=()"
    );

    // Absent here, and that is the assertion. This config is not production, so
    // a developer terminating TLS locally does not get `localhost` pinned for a
    // year by a browser they use for everything else -- and cannot serve the
    // retraction over the scheme it has started refusing.
    assert_eq!(home.headers().get("strict-transport-security"), None);

    let policy = home
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    for directive in [
        "default-src 'self'",
        "base-uri 'self'",
        "object-src 'none'",
        "frame-ancestors 'none'",
        "form-action 'self'",
        "style-src 'self'",
        "worker-src 'self' blob:",
    ] {
        assert!(policy.contains(directive), "{directive} missing: {policy}");
    }

    // A blob Worker inherits this document's policy, so what the runner needs
    // is what every script on the interview page is granted. It needs Pyodide
    // to compile wasm and nothing more: the candidate's JavaScript is built
    // into the worker rather than evaluated inside it. Both halves are pinned,
    // the grant and the refusal, because widening this back to `unsafe-eval`
    // would restore eval to the one page that renders the candidate's own text
    // and no test run would look any different.
    assert!(
        policy.contains("script-src 'self' blob: 'wasm-unsafe-eval'"),
        "{policy}"
    );
    assert!(!policy.contains("'unsafe-eval'"), "{policy}");

    // A .vrm is a GLB, so its textures are always bufferView-backed: GLTFLoader
    // mints a `blob:` URL per image and ImageBitmapLoader reads it with
    // `fetch`, which `connect-src` governs. Without `blob:` every texture fails
    // and the avatar falls back to its neutral panel with no stated reason.
    // Nothing else can catch this until a model ships, so it is pinned here.
    assert!(policy.contains("connect-src blob:"), "{policy}");

    // The page reaches LiveKit and Compiler Explorer directly, so each has to
    // be named or the interview cannot connect.
    assert!(
        policy.contains("wss://example.livekit.cloud:443"),
        "{policy}"
    );

    // LiveKit Cloud hands the SDK a regional host from `/settings/regions` and
    // it retries there, so the configured host alone leaves the retry blocked.
    assert!(policy.contains("https://*.livekit.cloud"), "{policy}");
    assert!(policy.contains("wss://*.livekit.cloud"), "{policy}");
    assert!(policy.contains("https://godbolt.org"), "{policy}");

    // Pyodide is served from web/vendor/pyodide/, so the CDN that used to
    // deliver the interpreter must not be reachable from the page at all.
    assert!(!policy.contains("cdn.jsdelivr.net"), "{policy}");
    // Loopback is a local-run affordance for the check harness only.
    assert!(policy.contains("http://127.0.0.1:*"), "{policy}");

    server.shutdown().await;
}

/// `web_service` is public, so its caller may not have used the binary's
/// configuration loader. Invalid endpoints must cost only their CSP sources.
#[tokio::test]
async fn public_web_service_omits_malformed_livekit_origins() {
    for (url, forbidden) in [
        ("wss://project.example;frame-src=*", "project.example"),
        ("wss://project.example:notaport", "project.example"),
        ("wss://[2001:db8::1", "2001:db8::1"),
        ("ftp://project.livekit.cloud", "livekit.cloud"),
    ] {
        let mut config = web_config();
        config.pool = primary_pool(url, "devkey", "devsecret");
        let (base, server) = spawn_web_server(config).await;
        let policy = http_client()
            .get(&base)
            .send()
            .await
            .unwrap()
            .headers()
            .get("content-security-policy")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(!policy.contains(forbidden), "{url}: {policy}");
        server.shutdown().await;
    }
}

#[tokio::test]
async fn production_policy_names_no_loopback_origins() {
    let (base, server) = spawn_web_server(WebServerConfig {
        web_dir: Path::new("web").to_path_buf(),
        production: true,
        compiler_explorer_enabled: false,
        ..web_config()
    })
    .await;

    let policy = http_client()
        .get(&base)
        .send()
        .await
        .unwrap()
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    assert!(!policy.contains("127.0.0.1"), "{policy}");
    assert!(!policy.contains("localhost"), "{policy}");
    assert!(
        !policy.contains("godbolt.org"),
        "a server that withdrew compiled runs must not permit the origin: {policy}"
    );

    server.shutdown().await;
}

/// The template Egress loads, and the policy it loads under.
///
/// The page is served by the same static fallback as the rest of `web/`, so
/// what is worth pinning is that the path task 1 fixed is reachable and that
/// the policy permits the socket the template has to open. A blocked signaling
/// socket is a recording of a page that never joined a room, and nothing in the
/// pipeline can tell that from an interview nobody spoke in.
#[tokio::test]
async fn the_recording_template_is_reachable_under_a_policy_that_permits_its_room() {
    let mut config = WebServerConfig {
        web_dir: Path::new("web").to_path_buf(),
        ..web_config()
    };
    let mut recording = recording_config();
    recording.livekit = Some(codetrial::config::RecordingLivekit {
        url: "wss://recording.livekit.cloud".to_string(),
        api_key: "key".to_string(),
        api_secret: "secret".to_string(),
    });
    config.recording = Some(recording);
    let (base, server) = spawn_web_server(config).await;

    let template = http_client()
        .get(format!("{base}{}", codetrial::recording::TEMPLATE_PATH))
        .send()
        .await
        .unwrap();
    assert_eq!(template.status(), 200);
    let policy = template
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let body = template.text().await.unwrap();
    assert!(body.contains(r#"id="recording-ready""#), "{body}");

    // Both schemes on the recording project's host, for the same reason the
    // pool's are: the SDK opens the socket at `wss://` and then calls the same
    // host over HTTPS.
    assert!(policy.contains("wss://recording.livekit.cloud"), "{policy}");
    assert!(
        policy.contains("https://recording.livekit.cloud"),
        "{policy}"
    );

    server.shutdown().await;
}

/// A page on another origin can give its own name a short TTL, repoint it at
/// 127.0.0.1 and then read a loopback server as same origin. The `Host` it
/// sends still carries that name, so a server told which names are its own
/// refuses the rest, `/healthz` included, and matches without case because a
/// host name has none.
#[tokio::test]
async fn an_allowed_hosts_list_refuses_every_other_host_name() {
    let (base, server) = spawn_web_server(WebServerConfig {
        allowed_hosts: vec!["desktop.test:3101".to_string()],
        ..web_config()
    })
    .await;
    let status = |host: &'static str| {
        let request = http_client()
            .get(format!("{base}/healthz"))
            .header(reqwest::header::HOST, host);
        async move { request.send().await.unwrap().status().as_u16() }
    };

    assert_eq!(status("Desktop.Test:3101").await, 200);
    assert_eq!(status("rebound.example:3101").await, 421);
    assert_eq!(status("desktop.test:3102").await, 421);

    server.shutdown().await;
}

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use codetrial::accounts::MAX_REPORTS_PER_USER;
use codetrial::config::{
    DEFAULT_DURATION_MIN, DEFAULT_WEB_DIR, MAX_DURATION_MIN, MIN_DURATION_MIN,
};
use codetrial::livekit::candidate_identity_matches;
use codetrial::runtime::{
    TOPIC_CODE_UPDATE, TOPIC_CONTROL, TOPIC_INTEGRITY, TOPIC_REPORT, TOPIC_TEST_RESULTS,
    TOPIC_TRANSCRIPTION,
};
use codetrial::token::{
    LivekitTokenInput, TOKEN_TTL_SECONDS, livekit_observer_token, livekit_room_admin_token,
    livekit_token,
};
use codetrial::web::{
    MAX_BODY_BYTES, MAX_REPORT_BYTES, REPLAY_RATE_LIMIT, RoomDispatcher, TOKEN_RATE_LIMIT,
    TokenConfig, WebServerConfig, initialize_account_database, login_config, static_file_meta,
    token_response,
};

type HmacSha256 = Hmac<Sha256>;

fn claims(token: &str) -> Value {
    serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(token.split('.').nth(1).unwrap())
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn token_has_livekit_video_grant_and_metadata() {
    let token = livekit_token(LivekitTokenInput {
        api_key: "key",
        api_secret: "secret",
        name: "Candidate",
        identity: "candidate-abc",
        room: "interview-abc",
        metadata: r#"{"problemId":"two-sum","durationMin":45}"#,
        now_seconds: 1000,
        agent: false,
    })
    .unwrap();
    let claims = claims(&token);

    assert_eq!(claims["iss"], "key");
    assert_eq!(claims["sub"], "candidate-abc");
    assert_eq!(claims["name"], "Candidate");
    assert_eq!(claims["exp"], 8200);
    assert_eq!(claims["video"]["room"], "interview-abc");
    assert_eq!(claims["video"]["roomJoin"], true);
    assert_eq!(claims["video"]["canPublish"], true);
    assert_eq!(claims["video"]["canSubscribe"], true);
    assert_eq!(claims["video"]["canPublishData"], true);
    assert_eq!(claims["video"]["agent"], false);
    assert_eq!(claims["video"]["canUpdateOwnMetadata"], false);
    assert!(claims.get("kind").is_none());
    assert_eq!(
        claims["metadata"],
        r#"{"problemId":"two-sum","durationMin":45}"#
    );
}

#[test]
fn agent_token_carries_agent_kind_and_metadata_grant() {
    let token = livekit_token(LivekitTokenInput {
        api_key: "devkey",
        api_secret: "devsecret",
        name: "Jim",
        identity: "interviewer-interview-fixed",
        room: "interview-fixed",
        metadata: r#"{"problemId":"merge-intervals","durationMin":30}"#,
        now_seconds: 2000,
        agent: true,
    })
    .unwrap();
    let claims = claims(&token);

    assert_eq!(claims["iss"], "devkey");
    assert_eq!(claims["sub"], "interviewer-interview-fixed");
    assert_eq!(claims["name"], "Jim");
    assert_eq!(claims["video"]["room"], "interview-fixed");
    assert_eq!(claims["video"]["agent"], true);
    assert_eq!(claims["video"]["canUpdateOwnMetadata"], true);
    assert_eq!(claims["kind"], "agent");
    assert_eq!(
        claims["metadata"],
        r#"{"problemId":"merge-intervals","durationMin":30}"#
    );
}

#[test]
fn room_admin_token_can_manage_only_the_target_room() {
    let token = livekit_room_admin_token("key", "secret", "interview-abc", 1000).unwrap();
    let claims = claims(&token);

    assert_eq!(claims["iss"], "key");
    assert_eq!(claims["sub"], "room-admin");
    assert_eq!(claims["exp"], 8200);
    assert_eq!(claims["video"]["room"], "interview-abc");
    assert_eq!(claims["video"]["roomAdmin"], true);
    assert!(claims["video"].get("roomJoin").is_none());
}

#[test]
fn observer_token_cannot_publish() {
    let claims = claims(
        &livekit_observer_token("key", "secret", "observer-abc", "interview-abc", 1000).unwrap(),
    );

    assert_eq!(claims["sub"], "observer-abc");
    assert_eq!(claims["video"]["room"], "interview-abc");
    assert_eq!(claims["video"]["roomJoin"], true);
    assert_eq!(claims["video"]["canPublish"], false);
    assert_eq!(claims["video"]["canPublishData"], false);
    assert_eq!(claims["video"]["canSubscribe"], true);
}

#[test]
fn token_response_matches_frontend_contract() {
    let response = token_response(
        &TokenConfig {
            api_key: "devkey",
            api_secret: "devsecret",
            server_url: "wss://example.livekit.cloud",
            recording_max_min: None,
        },
        br#"{"problemId":"merge-intervals","durationMin":120}"#,
        "interview-fixed",
        "candidate-fixed",
        2000,
    )
    .unwrap();
    let claims = claims(&response.token);

    assert_eq!(response.server_url, "wss://example.livekit.cloud");
    assert_eq!(response.room_name, "interview-fixed");
    assert_eq!(claims["name"], "Candidate");
    assert_eq!(claims["sub"], "candidate-fixed");
    assert_eq!(claims["video"]["room"], response.room_name);
    assert_eq!(
        claims["metadata"],
        serde_json::to_string(&json!({"problemId":"merge-intervals","durationMin":90,"interviewLoop":"coding_behavioral","interviewProfile":{"role":"","seniority":null,"targetCompany":""},"candidateIdentity":"candidate-fixed"})).unwrap()
    );
}

/// There is one interview now, so a mode is not a thing a browser can ask for.
///
/// A stale page or a hand-rolled client can still send one. It must not reach
/// the participant metadata under any spelling, because the agent no longer
/// reads it and a value sitting there would read as a setting that does
/// something.
#[test]
fn token_ignores_a_mode_a_stale_client_still_sends() {
    let config = TokenConfig {
        api_key: "devkey",
        api_secret: "devsecret",
        server_url: "wss://example.livekit.cloud",
        recording_max_min: None,
    };
    for body in [
        br#"{"mode":"practice"}"#.as_slice(),
        br#"{"mode":"scored"}"#.as_slice(),
        br#"{"mode":"forged"}"#.as_slice(),
        br#"{}"#.as_slice(),
    ] {
        let response = token_response(&config, body, "room", "candidate", 2_000).unwrap();
        let claims = claims(&response.token);
        let metadata: Value = serde_json::from_str(claims["metadata"].as_str().unwrap()).unwrap();
        assert!(
            metadata.get("mode").is_none(),
            "a mode reached the metadata for {body:?}"
        );
    }
}

#[test]
fn token_interview_loop_is_allowlisted_and_defaults_to_combined() {
    let config = TokenConfig {
        api_key: "key",
        api_secret: "secret",
        server_url: "wss://example.test",
        recording_max_min: None,
    };
    for (body, expected) in [
        (
            br#"{"interviewLoop":"coding_only"}"#.as_slice(),
            "coding_only",
        ),
        (
            br#"{"interviewLoop":"system_design"}"#.as_slice(),
            "coding_behavioral",
        ),
        (br#"{}"#.as_slice(), "coding_behavioral"),
    ] {
        let response = token_response(&config, body, "room", "candidate", 2_000).unwrap();
        let metadata: Value =
            serde_json::from_str(claims(&response.token)["metadata"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["interviewLoop"], expected);
    }
}

#[test]
fn token_profile_is_bounded_and_enum_validated_before_signed_metadata() {
    let response = token_response(
        &TokenConfig {
            api_key: "key",
            api_secret: "secret",
            server_url: "wss://example.test",
            recording_max_min: None,
        },
        serde_json::to_string(&json!({"interviewProfile": {
            "role": format!("  {}\n", "r".repeat(100)),
            "seniority": "staff",
            "targetCompany": "Example\u{0000} Co"
        }}))
        .unwrap()
        .as_bytes(),
        "room",
        "candidate",
        2_000,
    )
    .unwrap();
    let token_claims = claims(&response.token);
    let metadata: Value = serde_json::from_str(token_claims["metadata"].as_str().unwrap()).unwrap();
    assert_eq!(
        metadata["interviewProfile"]["role"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        80
    );
    assert_eq!(metadata["interviewProfile"]["seniority"], "staff");
    assert_eq!(metadata["interviewProfile"]["targetCompany"], "Example Co");

    let invalid = token_response(
        &TokenConfig {
            api_key: "key",
            api_secret: "secret",
            server_url: "wss://example.test",
            recording_max_min: None,
        },
        br#"{"interviewProfile":{"seniority":"founder","role":4}}"#,
        "room",
        "candidate",
        2_000,
    )
    .unwrap();
    let metadata: Value =
        serde_json::from_str(claims(&invalid.token)["metadata"].as_str().unwrap()).unwrap();
    assert!(metadata["interviewProfile"]["seniority"].is_null());
    assert_eq!(metadata["interviewProfile"]["role"], "");
}

#[test]
fn token_grounding_is_signed_only_after_valid_consent_and_shape() {
    let config = TokenConfig {
        api_key: "key",
        api_secret: "secret",
        server_url: "wss://example.test",
        recording_max_min: None,
    };
    let signed = token_response(&config, br#"{"interviewGrounding":{"consentVersion":1,"requirements":["Must know Rust"],"skills":["Rust"],"anchors":["Built a parser"]}}"#, "room", "candidate", 2_000).unwrap();
    let metadata: Value =
        serde_json::from_str(claims(&signed.token)["metadata"].as_str().unwrap()).unwrap();
    assert_eq!(
        metadata["interviewGrounding"]["anchors"][0],
        "Built a parser"
    );

    for body in [
        br#"{"interviewGrounding":{"requirements":[],"skills":[],"anchors":[]}}"#.as_slice(),
        br#"{"interviewGrounding":{"consentVersion":2,"requirements":[],"skills":[],"anchors":[]}}"#.as_slice(),
        br#"{"interviewGrounding":{"consentVersion":1,"requirements":"bad","skills":[],"anchors":[]}}"#.as_slice(),
    ] {
        let response = token_response(&config, body, "room", "candidate", 2_000).unwrap();
        let metadata: Value = serde_json::from_str(claims(&response.token)["metadata"].as_str().unwrap()).unwrap();
        assert!(metadata.get("interviewGrounding").is_none());
    }
}

#[test]
fn token_response_mints_the_candidate_identity() {
    let response = token_response(
        &TokenConfig {
            api_key: "devkey",
            api_secret: "devsecret",
            server_url: "wss://example.livekit.cloud",
            recording_max_min: None,
        },
        br#"{"candidateIdentity":"candidate-a1b2c3"}"#,
        "interview-fixed",
        "candidate-fallback",
        2000,
    )
    .unwrap();

    let claims = claims(&response.token);
    assert_eq!(claims["sub"], "candidate-fallback");
    assert_eq!(
        serde_json::from_str::<Value>(claims["metadata"].as_str().unwrap()).unwrap()["candidateIdentity"],
        "candidate-fallback"
    );
}

#[test]
fn token_response_ignores_a_supplied_candidate_identity() {
    let response = token_response(
        &TokenConfig {
            api_key: "devkey",
            api_secret: "devsecret",
            server_url: "wss://example.livekit.cloud",
            recording_max_min: None,
        },
        br#"{"candidateIdentity":"candidate-a1b2c3"}"#,
        "interview-fixed",
        "candidate-fallback",
        2000,
    )
    .unwrap();

    assert_eq!(claims(&response.token)["sub"], "candidate-fallback");
}

#[test]
fn token_duration_matches_current_frontend_clamp() {
    for (input, expected) in [
        (json!(1), 10),
        (json!(45), 45),
        (json!(120), 90),
        (json!("bad"), 45),
        (json!("30"), 30),
        (json!(" 30 "), 30),
        (json!(false), 45),
        (json!(true), 10),
    ] {
        let body = serde_json::to_vec(&json!({"durationMin": input})).unwrap();
        let response = token_response(
            &TokenConfig {
                api_key: "devkey",
                api_secret: "devsecret",
                server_url: "wss://example.livekit.cloud",
                recording_max_min: None,
            },
            &body,
            "interview-fixed",
            "candidate-fixed",
            2000,
        )
        .unwrap();
        let claims = claims(&response.token);

        assert_eq!(
            serde_json::from_str::<Value>(claims["metadata"].as_str().unwrap()).unwrap()["durationMin"],
            expected
        );
    }
}

#[test]
fn token_duration_preserves_fractional_frontend_metadata() {
    let response = token_response(
        &TokenConfig {
            api_key: "devkey",
            api_secret: "devsecret",
            server_url: "wss://example.livekit.cloud",
            recording_max_min: None,
        },
        br#"{"durationMin":42.8}"#,
        "interview-fixed",
        "candidate-fixed",
        2000,
    )
    .unwrap();
    let claims = claims(&response.token);

    assert_eq!(
        serde_json::from_str::<Value>(claims["metadata"].as_str().unwrap()).unwrap()["durationMin"],
        json!(42.8)
    );
}

#[test]
fn token_duration_stops_at_the_recording_cap() {
    for (recording_max_min, requested, expected) in [
        // The lobby offers sixty. A deployment recording at the default cap has
        // `stale_after` reap the recording at fifty, so the last ten minutes
        // are interview no artifact survives to cover.
        (Some(45), json!(60), json!(45)),
        // Under the cap is nobody's problem and stays untouched.
        (Some(45), json!(30), json!(30)),
        // A cap above the range the endpoint offers does not widen it.
        (Some(120), json!(100), json!(90)),
        // Nothing to outlive when this server does not record.
        (None, json!(60), json!(60)),
    ] {
        let body = serde_json::to_vec(&json!({"durationMin": requested})).unwrap();
        let response = token_response(
            &TokenConfig {
                api_key: "devkey",
                api_secret: "devsecret",
                server_url: "wss://example.livekit.cloud",
                recording_max_min,
            },
            &body,
            "interview-fixed",
            "candidate-fixed",
            2000,
        )
        .unwrap();
        let claims = claims(&response.token);

        assert_eq!(
            serde_json::from_str::<Value>(claims["metadata"].as_str().unwrap()).unwrap()["durationMin"],
            expected,
            "asked for {requested} under a cap of {recording_max_min:?}"
        );
    }
}

#[test]
fn token_duration_default_stops_at_the_recording_cap() {
    // A caller who names no length gets the default, and the default is a
    // length like any other. Handing back forty-five to a deployment that can
    // only record thirty loses the same stretch, for the want of a request.
    let response = token_response(
        &TokenConfig {
            api_key: "devkey",
            api_secret: "devsecret",
            server_url: "wss://example.livekit.cloud",
            recording_max_min: Some(30),
        },
        b"{}",
        "interview-fixed",
        "candidate-fixed",
        2000,
    )
    .unwrap();
    let claims = claims(&response.token);

    assert_eq!(
        serde_json::from_str::<Value>(claims["metadata"].as_str().unwrap()).unwrap()["durationMin"],
        30
    );
}

#[test]
fn token_duration_survives_a_recording_cap_under_the_floor() {
    // `f64::clamp` panics when its bounds cross, so a cap below the shortest
    // interview on offer has to land on the floor rather than in the handler.
    let response = token_response(
        &TokenConfig {
            api_key: "devkey",
            api_secret: "devsecret",
            server_url: "wss://example.livekit.cloud",
            recording_max_min: Some(1),
        },
        br#"{"durationMin":45}"#,
        "interview-fixed",
        "candidate-fixed",
        2000,
    )
    .unwrap();
    let claims = claims(&response.token);

    assert_eq!(
        serde_json::from_str::<Value>(claims["metadata"].as_str().unwrap()).unwrap()["durationMin"],
        10
    );
}

#[tokio::test]
async fn token_endpoint_rate_limits_a_noisy_client() {
    let (config, cookie, db_path) = signed_in_web_config("rate-limit");
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();
    let url = format!("{base}/api/token");
    let body = json!({"problemId":"two-sum","durationMin":45});

    for attempt in 1..=TOKEN_RATE_LIMIT {
        let response = client
            .post(&url)
            .header("cookie", &cookie)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            200,
            "request {attempt} should be allowed"
        );
    }

    let blocked = client
        .post(&url)
        .header("cookie", &cookie)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 429);
    assert_eq!(blocked.headers().get("retry-after").unwrap(), "60");

    server.abort();
    remove_database(db_path);
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

    let home = reqwest::get(&base).await.unwrap();

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

    // The runner evaluates candidate code in a blob Worker, which inherits this
    // document's policy, so dropping `unsafe-eval` would silently break test
    // runs rather than fail a check. Pinned so the trade-off stays deliberate.
    assert!(
        policy.contains("script-src 'self' blob: 'unsafe-eval'"),
        "{policy}"
    );

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

    server.abort();
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

    let policy = reqwest::get(&base)
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

    server.abort();
}

/// The resolved path alone. The resolver hands back the matched key and the
/// metadata beside it, and these tests care about neither.
async fn resolved(root: &Path, path: &str) -> Option<std::path::PathBuf> {
    static_file_meta(root, path).await.map(|(_, path, _)| path)
}

#[tokio::test]
async fn static_file_rejects_traversal_and_dotfiles() {
    assert!(
        resolved(Path::new("src/web"), "/../agent/.env")
            .await
            .is_none()
    );
    assert!(
        resolved(Path::new("src/web"), "/.env.local")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn static_file_falls_back_to_index_for_web_routes() {
    let root = std::env::temp_dir().join(format!("codetrial-web-test-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("index.html"), "").unwrap();

    assert_eq!(
        resolved(&root, "/interview").await,
        Some(root.join("index.html"))
    );

    fs::remove_dir_all(root).unwrap();
}

/// A path no directory occupies, so the handler has nothing on disk to find.
fn absent_web_dir(label: &str) -> std::path::PathBuf {
    unique_temp_path(label, "")
}

/// Process id and nanoseconds, so two tests in one binary and two binaries in
/// one suite cannot collide on it.
fn unique_temp_path(label: &str, suffix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "codetrial-{label}-{}-{}{suffix}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

/// The shipped binary has no `web/` beside it, so every asset a released
/// install serves takes this path. A 200 on `/` only proves resolution; what
/// the assertions below are for is the caching, which is the whole reason the
/// disk path grew a validator and a separate vendor policy. Serving 37 MB of
/// pinned wasm as `no-cache` with nothing to revalidate against would re-send
/// the tree on every navigation, and no status code would say so.
#[tokio::test]
async fn embedded_web_assets_serve_without_a_web_directory() {
    let mut config = web_config();
    config.web_dir = absent_web_dir("no-web");
    let (base, server) = spawn_web_server(config).await;

    let response = reqwest::get(&base).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("content-type").unwrap(),
        "text/html; charset=utf-8"
    );

    // `no-cache` revalidates, which is only affordable because the ETag below
    // turns the revalidation into a 304 rather than a resend.
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-cache");
    assert!(response.headers().get("etag").is_some());
    assert!(response.text().await.unwrap().contains("CodeTrial"));

    // Committed rather than fetched by `scripts/fetch-vendor.sh`, so the
    // assertion does not depend on whether the fetch has run here.
    let vendor = format!("{base}/vendor/avatar/three-vrm.js");
    let response = reqwest::get(&vendor).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("cache-control").unwrap(),
        "public, max-age=86400"
    );
    let etag = response.headers().get("etag").unwrap().clone();

    let client = reqwest::Client::new();
    let revalidated = client
        .get(&vendor)
        .header("if-none-match", etag.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(revalidated.status(), 304);

    // A 304 replaces the stored headers, so dropping either of these downgrades
    // the copy the browser already holds.
    assert_eq!(
        revalidated.headers().get("cache-control").unwrap(),
        "public, max-age=86400"
    );
    assert_eq!(revalidated.headers().get("etag").unwrap(), &etag);

    // Answered from the embedded length, never by reading out a body.
    let head = client.head(&vendor).send().await.unwrap();
    assert_eq!(head.status(), 200);
    assert!(head.headers().get("content-length").is_some());
    assert!(head.bytes().await.unwrap().is_empty());

    for route in ["/interview", "/interview/"] {
        let response = reqwest::get(&format!("{base}{route}")).await.unwrap();
        assert_eq!(response.status(), 200, "{route}");
        assert!(response.text().await.unwrap().contains("CodeTrial"));
    }

    // No separator or traversal spellings here. `url` collapses `..` before the
    // request leaves the client, and a debug build resolves the embed through
    // the filesystem, where the kernel collapses repeated separators whatever
    // the resolver does. Both would pass with the rules deleted.
    // `normalization_cannot_launder_a_refused_path` owns those cases against
    // the rule itself, and the release smoke step owns them against a binary
    // that really does match keys exactly.
    let response = reqwest::get(&format!("{base}/.git/config")).await.unwrap();
    assert_ne!(response.status(), 200);

    server.abort();
}

/// The fallback is per file, not per tree. Gating it on whether the web root
/// exists at all meant that anyone running the binary from a directory that
/// merely happens to contain a `web/` got a 404 for assets the executable was
/// carrying, which is the common shape: a checkout where `fetch-vendor.sh`
/// never ran.
#[tokio::test]
async fn embedded_assets_fill_gaps_in_a_partial_web_directory() {
    let root = absent_web_dir("partial-web");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("index.html"), "<!doctype html>disk wins").unwrap();

    let mut config = web_config();
    config.web_dir = root.clone();
    let (base, server) = spawn_web_server(config).await;

    // Present on disk, so disk answers even though the embed also has it.
    let response = reqwest::get(&base).await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "<!doctype html>disk wins");

    // Absent from this tree, so the embedded copy fills the gap.
    let response = reqwest::get(&format!("{base}/vendor/avatar/three-vrm.js"))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    // The same for a route with no extension, which is the harder half. Those
    // expand to four candidates ending in `index.html`, the single-page
    // fallback, so a disk store searched to exhaustion first would answer with
    // the one file it has and hand back the wrong page with a 200. Candidate
    // order has to beat store order.
    let interview = reqwest::get(&format!("{base}/interview"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !interview.contains("disk wins"),
        "an extensionless route must not fall back to a stale disk index: {}",
        &interview[..interview.len().min(200)]
    );
    assert!(
        interview.contains("<title>CodeTrial"),
        "it must be the real interview page: {}",
        &interview[..interview.len().min(200)]
    );

    server.abort();
    fs::remove_dir_all(root).unwrap();
}

/// A 200 here proves the file resolved, so there is no separate resolve-only
/// test restating the same names. These two headers are what decide whether the
/// detector loads at all and how long a stale copy survives an upgrade, and
/// neither is visible from the resolver.
#[tokio::test]
async fn vendored_assets_are_served_typed_and_cached() {
    let mut config = web_config();
    config.web_dir = std::path::PathBuf::from(DEFAULT_WEB_DIR);
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();

    for (file, content_type) in [
        // `WebAssembly.instantiateStreaming` refuses anything else.
        ("face_detection_solution_wasm_bin.wasm", "application/wasm"),
        (
            "face_detection_short_range.tflite",
            "application/octet-stream",
        ),
        ("face_detection_short.binarypb", "application/octet-stream"),
        ("face_detection.js", "text/javascript; charset=utf-8"),
    ] {
        let response = client
            .get(format!("{base}/vendor/face-detection/{file}"))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200, "{file}");
        assert_eq!(response.headers()["content-type"], content_type, "{file}");

        // Long enough to skip revalidating 11.8 MB on a reload, short enough
        // that an in-place vendor upgrade reaches a browser that cached it.
        assert_eq!(response.headers()["cache-control"], "public, max-age=86400");
        assert!(response.headers().contains_key("etag"), "{file}");
    }

    // Everything outside the vendor tree keeps revalidating.
    let response = client
        .get(format!("{base}/interview.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-cache");

    // HEAD has to answer from metadata rather than by producing a body and
    // throwing it away. Axum routes HEAD to the same handler, so without this
    // the largest vendored file is read into memory once per probe.
    let head = client
        .head(format!("{base}/vendor/face-detection/face_detection.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 200);
    assert!(head.headers().contains_key("content-length"));
    assert!(head.headers().contains_key("etag"));
    let etag = head.headers()["etag"].clone();
    assert_eq!(head.bytes().await.unwrap().len(), 0);

    // A conditional HEAD is a revalidation, so it answers 304 rather than
    // claiming the cached copy was replaced.
    let revalidated = client
        .head(format!("{base}/vendor/face-detection/face_detection.js"))
        .header("if-none-match", etag)
        .send()
        .await
        .unwrap();
    assert_eq!(revalidated.status(), 304);

    // And a HEAD for something absent is still a 404 rather than an empty 200.
    let missing = client
        .head(format!("{base}/vendor/avatar/no-such-model.vrm"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    // The retired model URL, over HTTP. `static_candidates` refuses it outright
    // and `retired_avatar_model_is_never_served_from_disk` pins that refusal
    // directly, so this is the end-to-end half: the refusal survives routing,
    // the disk-first override, and the embedded fallback. It is deliberately
    // not the proof that the model is unembedded, because it cannot be, and
    // `no_model_is_embedded_in_the_binary` is where that lives.
    let unserved = client
        .get(format!("{base}/vendor/avatar/jim.vrm"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        unserved.status(),
        404,
        "the avatar model must not be served from this tree"
    );

    // The tree compresses: the wasm is 11 MB and the ratio there is real.
    let compressed = client
        .get(format!("{base}/vendor/face-detection/face_detection.js"))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert_eq!(compressed.headers()["content-encoding"], "gzip");

    server.abort();
}

/// Pins a file's modification time so a cache test can state the exact case it
/// means instead of hoping the clock cooperates.
fn set_modified(path: &Path, at: SystemTime) {
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(at)
        .unwrap();
}

#[tokio::test]
async fn static_server_serves_health_fixture_and_missing_asset() {
    let root = std::env::temp_dir().join(format!(
        "codetrial-web-server-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("index.html"), "<h1>CodeTrial</h1>").unwrap();
    fs::write(root.join("app.js"), "globalThis.loaded = true;").unwrap();

    let (base, server) = spawn_web_server(WebServerConfig {
        web_dir: root.clone(),
        ..web_config()
    })
    .await;
    let client = reqwest::Client::new();

    let health = client.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(health.status(), 200);
    assert_eq!(health.text().await.unwrap(), "ok\n");

    let asset = client.get(format!("{base}/app.js")).send().await.unwrap();
    assert_eq!(asset.status(), 200);
    assert_eq!(
        asset.headers().get("content-type").unwrap(),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(asset.headers().get("cache-control").unwrap(), "no-cache");
    let etag = asset.headers().get("etag").unwrap().clone();
    assert!(
        etag.to_str().unwrap().starts_with("W/\""),
        "the validator is weak, so it still matches once responses are compressed"
    );
    assert_eq!(asset.text().await.unwrap(), "globalThis.loaded = true;");

    // `web/` is 13 MB, so revalidation has to cost a 304 rather than a resend.
    let revalidated = client
        .get(format!("{base}/app.js"))
        .header("if-none-match", &etag)
        .send()
        .await
        .unwrap();
    assert_eq!(revalidated.status(), 304);

    // RFC 9110 requires a 304 to repeat the validator and the `Vary` its 200
    // carried, so a shared cache still knows what it confirmed and which
    // encoding it holds.
    assert_eq!(revalidated.headers().get("etag").unwrap(), &etag);
    assert_eq!(
        revalidated.headers().get("vary").unwrap(),
        "accept-encoding"
    );
    assert_eq!(revalidated.text().await.unwrap(), "");

    // The validator is modification time and length, so the case that decides
    // whether it works is the one where length cannot help: an edit inside the
    // same second that leaves the file exactly as long. Both timestamps are
    // pinned rather than taken from the clock, because two real writes almost
    // always land on different nanoseconds anyway, and then this passes without
    // ever exercising sub-second resolution. `= true;` and `= null;` are both
    // 25 bytes, so length is identical on purpose.
    let app_js = root.join("app.js");
    let same_second = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    set_modified(&app_js, same_second);
    let pinned = client.get(format!("{base}/app.js")).send().await.unwrap();
    let pinned_etag = pinned.headers().get("etag").unwrap().clone();

    fs::write(&app_js, "globalThis.loaded = null;").unwrap();
    set_modified(&app_js, same_second + Duration::from_nanos(1));

    let changed = client
        .get(format!("{base}/app.js"))
        .header("if-none-match", &pinned_etag)
        .send()
        .await
        .unwrap();
    assert_eq!(
        changed.status(),
        200,
        "a same-length edit one nanosecond later must still invalidate the cache"
    );
    assert_eq!(changed.text().await.unwrap(), "globalThis.loaded = null;");

    // A stale validator must not be honored, or an edit would never reach the
    // browser that cached the file before it.
    let changed = client
        .get(format!("{base}/app.js"))
        .header("if-none-match", "W/\"0-1\"")
        .send()
        .await
        .unwrap();
    assert_eq!(changed.status(), 200);
    assert_eq!(changed.text().await.unwrap(), "globalThis.loaded = null;");

    let missing = client
        .get(format!("{base}/missing.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    server.abort();
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn runtime_config_can_disable_compiled_language_runs() {
    let (enabled_base, enabled_server) = spawn_web_server(web_config()).await;
    let client = reqwest::Client::new();

    let enabled = client
        .get(format!("{enabled_base}/runtime-config.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(enabled.status(), 200);
    assert_eq!(
        enabled.headers().get("content-type").unwrap(),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(enabled.headers().get("cache-control").unwrap(), "no-store");
    assert_eq!(
        enabled.text().await.unwrap(),
        "globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = true;\nglobalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = \"https://godbolt.org\";\nglobalThis.CODETRIAL_RECORDING_ENABLED = false;\nglobalThis.CODETRIAL_CONSENT_VERSION = \"2026-08-21\";\nglobalThis.CODETRIAL_REPLAY_VERSION = 1;\n"
    );
    enabled_server.abort();

    let mut disabled_config = web_config();
    disabled_config.compiler_explorer_enabled = false;
    let (disabled_base, disabled_server) = spawn_web_server(disabled_config).await;
    let disabled = client
        .get(format!("{disabled_base}/runtime-config.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(disabled.status(), 200);
    assert_eq!(
        disabled.text().await.unwrap(),
        "globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = false;\nglobalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = \"\";\nglobalThis.CODETRIAL_RECORDING_ENABLED = false;\nglobalThis.CODETRIAL_CONSENT_VERSION = \"2026-08-21\";\nglobalThis.CODETRIAL_REPLAY_VERSION = 1;\n"
    );
    disabled_server.abort();

    // The page cannot know whether to show the consent step without being told,
    // and it must not guess: a candidate shown no notice on a recording server
    // is the failure the whole feature exists to prevent.
    let mut recording_config_values = web_config();
    recording_config_values.recording = Some(recording_config());
    let (recording_base, recording_server) = spawn_web_server(recording_config_values).await;
    let recording = client
        .get(format!("{recording_base}/runtime-config.js"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(recording.contains("globalThis.CODETRIAL_RECORDING_ENABLED = true;"));
    assert!(recording.contains(&format!(
        "globalThis.CODETRIAL_CONSENT_VERSION = \"{}\";",
        codetrial::recording::CONSENT_VERSION
    )));
    recording_server.abort();
}

#[tokio::test]
async fn static_home_markup_matches_frontend_contract() {
    let (base, server) = spawn_web_server(WebServerConfig {
        web_dir: Path::new("web").to_path_buf(),
        github_client_id: None,
        github_client_secret: None,
        session_secret: None,
        db_path: None,
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: "interview".to_string(),
        fixed_room_name: None,
        production: false,
        compiler_explorer_enabled: true,
        trusted_proxy_hops: 0,
        recording: None,
        pool: Default::default(),
        probe_provider_quota: false,
    })
    .await;
    let html = reqwest::get(base).await.unwrap().text().await.unwrap();

    for text in [
        "Practice a live technical interview",
        "30 min",
        "45 min",
        "60 min",
        "Start interview",
    ] {
        assert!(html.contains(text), "missing home contract text: {text}");
    }

    // Every problem in the bank must reach the lobby. Read the titles from the
    // bank rather than restating them, so adding a problem cannot pass here by
    // being forgotten in two places at once.
    let problems = browser_problem_bank();
    for problem in problems.as_array().unwrap() {
        let title = problem["title"].as_str().unwrap();
        assert!(
            html.contains(title),
            "lobby is missing problem card: {title}"
        );
    }

    server.abort();
}

#[tokio::test]
async fn static_interview_markup_exposes_offline_surface() {
    let (base, server) = spawn_web_server(WebServerConfig {
        web_dir: Path::new("web").to_path_buf(),
        github_client_id: None,
        github_client_secret: None,
        session_secret: None,
        db_path: None,
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: "interview".to_string(),
        fixed_room_name: None,
        production: false,
        compiler_explorer_enabled: true,
        trusted_proxy_hops: 0,
        recording: None,
        pool: Default::default(),
        probe_provider_quota: false,
    })
    .await;
    let html = reqwest::get(format!("{base}/interview"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    for text in [
        "CODETRIAL",
        "Problem",
        "Transcript",
        "Python 3",
        "JavaScript",
        "C",
        "C++",
        "Java",
        "Run tests",
        "End interview",
        "Test results",
    ] {
        assert!(
            html.contains(text),
            "missing interview contract text: {text}"
        );
    }

    server.abort();
}

#[test]
fn static_interview_script_keeps_data_topic_publish_contract() {
    let topics = fs::read_to_string("web/lib.js").unwrap();
    let source = fs::read_to_string("web/interview.js").unwrap();

    // The browser topic table is compared against the Rust constants
    // themselves, so renaming a topic on either side fails here rather than
    // silently splitting the two halves of the data channel. The payload shapes
    // behind these topics are asserted in tests/browser/lib.test.js.
    for (key, topic) in [
        ("code", TOPIC_CODE_UPDATE),
        ("control", TOPIC_CONTROL),
        ("integrity", TOPIC_INTEGRITY),
        ("report", TOPIC_REPORT),
        ("tests", TOPIC_TEST_RESULTS),
        ("transcript", TOPIC_TRANSCRIPTION),
    ] {
        let entry = format!("{key}: {topic:?}");
        assert!(
            topics.contains(&entry),
            "web/lib.js topics must carry {entry}"
        );
    }
    assert!(
        source.contains("publishData(new TextEncoder().encode(JSON.stringify(payload))"),
        "publish must send JSON-encoded payloads on the data channel"
    );
}

#[test]
fn static_interview_script_keeps_transcript_and_report_contract() {
    let source = fs::read_to_string("web/interview.js").unwrap();
    let transcript = source_block(
        &source,
        "async function consumeTranscript",
        "async function toggleMicrophone",
    );
    let guard = source_block(
        &source,
        "room.on(livekit.RoomEvent.DataReceived",
        "room.on(livekit.RoomEvent.ParticipantAttributesChanged",
    );
    let report = source_block(
        &source,
        "function receiveReport",
        "function playRemoteAudio",
    );

    // acceptsReport itself is covered by tests/browser/lib.test.js; this only
    // pins that the handler still consults it before touching the report.
    assert!(
        guard.contains("if (!acceptsReport(topic, participant)) return"),
        "report handler must stay gated on acceptsReport"
    );

    // LiveKit stream attribute names, which the Rust side sets in
    // transcript_stream_options.
    for snippet in [
        r#"attrs["lk.segment_id"]"#,
        r#"attrs["lk.transcription_final"] === "true""#,
        r#"participant?.identity === room.localParticipant.identity ? "you" : "interviewer""#,
        "updateTranscriptSegment(id, speaker, text",
    ] {
        assert!(
            transcript.contains(snippet),
            "missing static transcript contract: {snippet}"
        );
    }

    // The report card and markdown export are asserted behaviorally in
    // tests/browser/render.test.js. What only Rust can check is that the
    // browser still routes a report through the sanitizer before rendering it.
    for snippet in [
        "sanitizeReport(JSON.parse(new TextDecoder().decode(payload)))",
        "setLocalAudioEnabled(false)",
        "saveHistory()",
        "renderReport()",
        "room.disconnect()",
        "state.room = null",
    ] {
        assert!(
            report.contains(snippet),
            "missing static report receive contract: {snippet}"
        );
    }

    // The report card and the markdown export are asserted behaviorally in
    // tests/browser/render.test.js; what only Rust can see is that the browser
    // still routes both through the sanitizer before rendering.
}

#[test]
fn static_interview_script_leaves_candidate_identity_to_the_server() {
    let source = fs::read_to_string("web/interview.js").unwrap();

    assert!(
        source
            .contains("JSON.stringify({ problemId: problem.id, durationMin, interviewId, interviewLoop, interviewProfile, ...(interviewGrounding ? { interviewGrounding } : {}) })")
    );
    assert!(!source.contains("candidateIdentity"));
}

#[test]
fn static_problem_bank_and_judges_cover_each_problem() {
    let problems = browser_problem_bank();
    let judges = browser_judges();
    let problems = problems.as_array().unwrap();
    let judges = judges.as_object().unwrap();

    let supported_arg_type = |arg_type: &serde_json::Value| {
        arg_type.is_null()
            || matches!(
                arg_type.as_str(),
                Some(
                    "linkedList"
                        | "linkedListArray"
                        | "randomList"
                        | "graphNode"
                        | "binaryTree"
                        | "nextTree"
                        | "treeNodeValue"
                        | "cyclePos",
                )
            )
    };
    let supported_signature_type = |signature_type: &serde_json::Value| {
        matches!(
            signature_type.as_str(),
            Some(
                "boolean"
                    | "character[][]"
                    | "double"
                    | "double[]"
                    | "integer"
                    | "integer[]"
                    | "integer[][]"
                    | "list<double>"
                    | "list<integer>"
                    | "list<list<integer>>"
                    | "list<list<string>>"
                    | "list<string>"
                    | "ListNode"
                    | "ListNode[]"
                    | "Node"
                    | "string"
                    | "string[]"
                    | "TreeNode"
                    | "void",
            )
        )
    };

    for problem in problems {
        let id = problem["id"].as_str().unwrap();
        let spec = judges
            .get(id)
            .unwrap_or_else(|| panic!("missing judge for {id}"));
        assert!(
            spec["cases"].as_array().unwrap().len() >= 3,
            "not enough judge cases for {id}"
        );
        for language in ["python", "javascript", "c", "cpp", "java"] {
            assert!(
                problem["starterCode"].get(language).is_some(),
                "missing {language} starter for {id}"
            );
        }
        if spec["kind"] == "class" {
            assert!(
                spec["className"]
                    .as_str()
                    .is_some_and(|name| !name.is_empty()),
                "missing class name for {id}"
            );
        }
        if spec["kind"] == "function" {
            let param_names = spec["paramNames"]
                .as_array()
                .unwrap_or_else(|| panic!("paramNames must be an array for {id}"));
            let param_types = spec["paramTypes"]
                .as_array()
                .unwrap_or_else(|| panic!("paramTypes must be an array for {id}"));
            assert_eq!(
                param_names.len(),
                param_types.len(),
                "paramNames length must match paramTypes length for {id}"
            );
            for case in spec["cases"].as_array().unwrap() {
                assert_eq!(
                    param_types.len(),
                    case["input"].as_array().unwrap().len(),
                    "paramTypes length must match case input arity for {id}"
                );
            }
            for param_name in param_names {
                assert!(
                    param_name.as_str().is_some_and(|name| !name.is_empty()),
                    "empty paramName for {id}"
                );
            }
            for param_type in param_types {
                assert!(
                    supported_signature_type(param_type),
                    "unsupported paramType for {id}: {param_type:?}"
                );
            }
            assert!(
                supported_signature_type(&spec["returnType"]),
                "unsupported returnType for {id}: {:?}",
                spec["returnType"]
            );
        }
        if let Some(arg_types) = spec.get("argTypes") {
            let arg_types = arg_types
                .as_array()
                .unwrap_or_else(|| panic!("argTypes must be an array for {id}"));
            for case in spec["cases"].as_array().unwrap() {
                assert_eq!(
                    arg_types.len(),
                    case["input"].as_array().unwrap().len(),
                    "argTypes length must match case input arity for {id}"
                );
            }
            for arg_type in arg_types {
                assert!(
                    supported_arg_type(arg_type),
                    "unsupported argType for {id}: {arg_type:?}"
                );
            }
        }
        if let Some(arg_types) = spec.get("constructorArgTypes") {
            let arg_types = arg_types
                .as_array()
                .unwrap_or_else(|| panic!("constructorArgTypes must be an array for {id}"));
            for case in spec["cases"].as_array().unwrap() {
                let args = &case["input"].as_array().unwrap()[1].as_array().unwrap()[0];
                assert_eq!(
                    arg_types.len(),
                    args.as_array().unwrap().len(),
                    "constructorArgTypes length must match constructor arity for {id}"
                );
            }
            for arg_type in arg_types {
                assert!(
                    supported_arg_type(arg_type),
                    "unsupported constructorArgType for {id}: {arg_type:?}"
                );
            }
        }
        if matches!(
            spec.get("outputType").and_then(|value| value.as_str()),
            Some(
                "linkedList" | "randomList" | "graphNode" | "binaryTree" | "nextTree" | "quadTree"
            )
        ) {
            for case in spec["cases"].as_array().unwrap() {
                assert!(
                    case["expected"].as_array().is_some(),
                    "list output expected value must be an array for {id}"
                );
            }
        }
        if let Some(output_param) = spec.get("outputParam") {
            let output_param = output_param
                .as_u64()
                .unwrap_or_else(|| panic!("outputParam must be a number for {id}"));
            for case in spec["cases"].as_array().unwrap() {
                assert!(
                    case["input"]
                        .as_array()
                        .is_some_and(|input| (output_param as usize) < input.len()),
                    "outputParam out of bounds for {id}"
                );
            }
        }
        if let Some(output_param) = spec.get("outputPrefixParam") {
            let output_param = output_param
                .as_u64()
                .unwrap_or_else(|| panic!("outputPrefixParam must be a number for {id}"));
            for case in spec["cases"].as_array().unwrap() {
                assert!(
                    case["input"]
                        .as_array()
                        .is_some_and(|input| (output_param as usize) < input.len()),
                    "outputPrefixParam out of bounds for {id}"
                );
                assert!(
                    case["expected"].as_array().is_some(),
                    "outputPrefixParam expected output must be an array for {id}"
                );
            }
        }
    }

    // Problems whose judges cannot be plain equality need their custom checker,
    // and the tricky inputs must stay in the fixture set.
    assert_eq!(judges["two-sum"]["checker"], "twoSum");
    assert_eq!(
        judges["longest-palindromic-substring"]["checker"],
        "palindrome"
    );
    assert_eq!(judges["course-schedule-ii"]["checker"], "topologicalOrder");
    assert_eq!(
        judges["convert-sorted-array-to-binary-search-tree"]["checker"],
        "balancedBst"
    );
    for (id, label) in [
        ("two-sum", "duplicates"),
        ("merge-sorted-array", "duplicates"),
        ("remove-element", "remove every"),
        ("remove-duplicates-from-sorted-array", "several duplicate"),
        ("remove-duplicates-from-sorted-array-ii", "single repeated"),
        ("majority-element", "not the first"),
        ("rotate-array", "k larger"),
        ("best-time-to-buy-and-sell-stock", "decreasing"),
        ("best-time-to-buy-and-sell-stock-ii", "multiple small rises"),
        ("jump-game", "late unreachable"),
        ("jump-game-ii", "greedy window"),
        ("h-index", "h capped"),
        ("insert-delete-getrandom-o1", "removed value"),
        ("product-of-array-except-self", "two zeros"),
        ("gas-station", "wraparound"),
        ("candy", "plateau between"),
        ("trapping-rain-water", "right boundary"),
        ("roman-to-integer", "compound subtractive"),
        ("integer-to-roman", "compound subtractive"),
        ("length-of-last-word", "trailing spaces"),
        ("longest-common-prefix", "empty string"),
        ("reverse-words-in-a-string", "collapse internal spaces"),
        ("zigzag-conversion", "single row"),
        (
            "find-the-index-of-the-first-occurrence-in-a-string",
            "later occurrence",
        ),
        ("text-justification", "uneven spaces go left"),
        ("valid-palindrome", "digits count"),
        ("is-subsequence", "not substring"),
        ("container-with-most-water", "interior best"),
        ("two-sum-ii-input-array-is-sorted", "duplicate values"),
        ("3sum", "many duplicates"),
        ("happy-number", "cycle at four"),
        (
            "longest-substring-without-repeating-characters",
            "left pointer",
        ),
        ("minimum-window-substring", "duplicate required"),
        (
            "substring-with-concatenation-of-all-words",
            "duplicate words",
        ),
        ("minimum-size-subarray-sum", "no qualifying window"),
        ("valid-sudoku", "duplicate in box only"),
        ("spiral-matrix", "single column"),
        ("rotate-image", "negative values"),
        ("set-matrix-zeroes", "first column marker"),
        ("game-of-life", "simultaneous blinker"),
        ("ransom-note", "insufficient multiplicity"),
        ("isomorphic-strings", "two sources one target"),
        ("word-pattern", "many pattern letters share one word"),
        ("valid-anagram", "same letters wrong multiplicity"),
        ("group-anagrams", "duplicate words preserved"),
        ("contains-duplicate-ii", "k zero"),
        ("longest-consecutive-sequence", "duplicates in long run"),
        ("summary-ranges", "negative to positive range"),
        ("insert-interval", "touching endpoints merge"),
        (
            "minimum-number-of-arrows-to-burst-balloons",
            "touching endpoints share arrow",
        ),
        ("simplify-path", "dot names are directories"),
        ("min-stack", "duplicate minimum survives one pop"),
        (
            "evaluate-reverse-polish-notation",
            "negative division truncates toward zero",
        ),
        ("basic-calculator", "nested subtraction"),
        ("linked-list-cycle", "tail points to itself"),
        ("add-two-numbers", "carry extends result"),
        ("merge-two-sorted-lists", "negative values"),
        ("copy-list-with-random-pointer", "duplicate values"),
        ("reverse-linked-list-ii", "negative values"),
        ("reverse-nodes-in-k-group", "k equals one"),
        ("remove-nth-node-from-end-of-list", "remove head"),
        (
            "remove-duplicates-from-sorted-list-ii",
            "all values duplicated",
        ),
        ("rotate-list", "k larger than length"),
        ("partition-list", "relative order preserved"),
        ("maximum-depth-of-binary-tree", "left skew depth four"),
        ("same-tree", "same values different null side"),
        ("invert-binary-tree", "sparse tree keeps null positions"),
        ("symmetric-tree", "cross sparse mirror"),
        (
            "construct-binary-tree-from-preorder-and-inorder-traversal",
            "left skew",
        ),
        (
            "construct-binary-tree-from-inorder-and-postorder-traversal",
            "right skew",
        ),
        (
            "populating-next-right-pointers-in-each-node-ii",
            "missing middle children",
        ),
        (
            "flatten-binary-tree-to-linked-list",
            "branching preorder chain",
        ),
        ("path-sum", "prefix sum is not enough"),
        ("sum-root-to-leaf-numbers", "skewed digits"),
        (
            "binary-tree-maximum-path-sum",
            "all negative picks one node",
        ),
        ("binary-search-tree-iterator", "left skew bst"),
        ("count-complete-tree-nodes", "partial final level"),
        (
            "lowest-common-ancestor-of-a-binary-tree",
            "ancestor is one target",
        ),
        (
            "binary-tree-right-side-view",
            "left depth visible after right ends",
        ),
        ("average-of-levels-in-binary-tree", "mixed signs average"),
        (
            "binary-tree-level-order-traversal",
            "sparse keeps left to right",
        ),
        (
            "binary-tree-zigzag-level-order-traversal",
            "four levels alternate",
        ),
        (
            "minimum-absolute-difference-in-bst",
            "minimum not parent child",
        ),
        ("kth-smallest-element-in-a-bst", "left skew middle"),
        (
            "validate-binary-search-tree",
            "deep descendant violates ancestor",
        ),
        ("surrounded-regions", "border connected region stays"),
        ("clone-graph", "chain graph deep copy"),
        ("evaluate-division", "disconnected components"),
        ("course-schedule", "long cycle"),
        ("course-schedule-ii", "branching prerequisites"),
        ("snakes-and-ladders", "unreachable trap"),
        ("minimum-genetic-mutation", "end missing from bank"),
        ("word-ladder", "shorter route through decoy"),
        ("implement-trie-prefix-tree", "prefix is not word"),
        (
            "design-add-and-search-words-data-structure",
            "dot matches exactly one letter",
        ),
        ("word-search-ii", "duplicate board paths return word once"),
        (
            "letter-combinations-of-a-phone-number",
            "four choices digit",
        ),
        ("combinations", "choose all numbers"),
        ("permutations", "negative value"),
        ("combination-sum", "unsorted candidates"),
        ("n-queens-ii", "two queens impossible"),
        ("generate-parentheses", "four pairs"),
        ("word-search", "cannot reuse cell"),
        (
            "convert-sorted-array-to-binary-search-tree",
            "two values allow either root",
        ),
        ("merge-intervals", "unsorted"),
        ("valid-parentheses", "interleaved"),
    ] {
        let labels = judges[id]["cases"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|case| case["label"].as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            labels.contains(label),
            "{id} judge fixtures should cover the {label} case, got: {labels}"
        );
    }
}

/// The rubric (optimal approach, pitfalls) must stay server-side, so the Rust
/// table cannot simply be generated from the browser bank. Cross-check the
/// fields both sides duplicate instead, so drift fails here rather than
/// silently giving the agent a different problem than the candidate sees.
#[test]
fn rust_problem_bank_matches_the_browser_problem_bank() {
    let browser = browser_problem_bank();
    let browser = browser.as_array().unwrap();

    assert_eq!(
        browser.len(),
        codetrial::agent::PROBLEMS.len(),
        "problem count differs between web/problems/ and src/agent.rs"
    );

    // Matched by id, not position: the browser list is in lobby display order.
    for problem in codetrial::agent::PROBLEMS {
        let entry = browser
            .iter()
            .find(|entry| entry["id"].as_str() == Some(problem.id))
            .unwrap_or_else(|| panic!("web/problems/ is missing {}", problem.id));
        assert_eq!(
            entry["title"].as_str(),
            Some(problem.title),
            "title differs for {}",
            problem.id
        );
        assert_eq!(
            entry["difficulty"].as_str(),
            Some(problem.difficulty),
            "difficulty differs for {}",
            problem.id
        );
    }
}

#[tokio::test]
async fn server_check_accepts_running_rust_server() {
    let (mut config, cookie, db_path) = signed_in_web_config("server-check");
    config.web_dir = Path::new("web").to_path_buf();

    // A credentialled pool, because an empty one is no longer a server that can
    // exist: `run_web` refuses to start without LiveKit credentials, so the
    // check asserts `/api/token` mints rather than 500s.
    config.pool = primary_pool(
        "wss://example.livekit.cloud",
        "server-check-key",
        "server-check-secret",
    );
    let (base, server) = spawn_web_server(config).await;
    let output = tokio::task::spawn_blocking(move || {
        Command::new("scripts/server-check.sh")
            .env("CODETRIAL_WEB_URL", base)
            .env("SERVER_CHECK_SESSION_COOKIE", cookie)
            .output()
    })
    .await
    .unwrap()
    .expect("frontend check should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    server.abort();
    remove_database(db_path);
}

/// The check signs itself in through `/api/login` rather than being handed a
/// cookie, so it works against a server it did not start.
#[tokio::test]
async fn server_check_signs_itself_in_against_an_external_server() {
    let (mut config, _, db_path) = signed_in_web_config("server-check-cookie");
    config.web_dir = Path::new("web").to_path_buf();
    config.pool = primary_pool(
        "wss://example.livekit.cloud",
        "server-check-key",
        "server-check-secret",
    );
    let (base, server) = spawn_web_server(config).await;
    let output = tokio::task::spawn_blocking(move || {
        Command::new("scripts/server-check.sh")
            .env("CODETRIAL_WEB_URL", base)
            .env_remove("SERVER_CHECK_SESSION_COOKIE")
            .output()
    })
    .await
    .unwrap()
    .expect("frontend check should run");

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    server.abort();
    remove_database(db_path);
}

#[tokio::test]
#[ignore = "browser-dependent: hermetic and about 30s idle, but a real browser \
            under a loaded machine has been measured failing on page.goto past \
            a two minute navigation timeout. CI runs it as its own step; \
            locally, `cargo test --test web -- --ignored` when the machine is \
            not busy"]
async fn browser_check_accepts_running_rust_server_offline_interview() {
    let (mut config, cookie, db_path) = signed_in_web_config("browser-offline");
    config.web_dir = Path::new("web").to_path_buf();
    config.pool = Default::default();
    let (base, server) = spawn_web_server(config).await;
    let output = tokio::task::spawn_blocking(move || {
        Command::new("scripts/browser-check.sh")
            .env("CODETRIAL_WEB_URL", base)
            .env("BROWSER_CHECK_AGENT", "offline")
            .env("BROWSER_CHECK_SESSION_COOKIE", cookie)
            // The mock, not godbolt.org. The live service is what the default
            // reaches for, and it turned this into a four minute test that a
            // network hiccup could fail: the mock runs the same editor and
            // results pipeline against fixed responses in a fraction of it.
            // Whether godbolt's API still looks the way this code expects is a
            // real question, but it is not one a commit should be blocked on.
            .env("BROWSER_CHECK_COMPILER_EXPLORER_BASE_URL", "mock")
            .output()
    })
    .await
    .unwrap()
    .expect("browser check should run");

    // Exit 3 is browser-check.sh saying Playwright is not installed. Skipped
    // rather than failed, matching tests/browser/face-detector.test.js: the
    // suite has to stay runnable without an 800 MB download. CI installs
    // Chromium, so CI never takes this arm.
    if output.status.code() == Some(3) {
        eprintln!(
            "skipping offline browser check: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        server.abort();
        remove_database(db_path);
        return;
    }

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    server.abort();
    remove_database(db_path);
}

#[tokio::test]
#[ignore = "credentialed browser check; requires LiveKit and Google env"]
async fn browser_check_accepts_running_rust_server_with_rust_agent() {
    let (mut config, cookie, db_path) = signed_in_web_config("browser-rust");
    config.web_dir = Path::new("web").to_path_buf();
    config.pool = primary_pool(
        &std::env::var("LIVEKIT_URL").expect("set LIVEKIT_URL"),
        &std::env::var("LIVEKIT_API_KEY").expect("set LIVEKIT_API_KEY"),
        &std::env::var("LIVEKIT_API_SECRET").expect("set LIVEKIT_API_SECRET"),
    );
    let (base, server) = spawn_web_server(config).await;
    let output = tokio::task::spawn_blocking(move || {
        Command::new("scripts/browser-check.sh")
            .env("CODETRIAL_WEB_URL", base)
            .env("BROWSER_CHECK_AGENT", "rust")
            .env("BROWSER_CHECK_SESSION_COOKIE", cookie)
            .output()
    })
    .await
    .unwrap()
    .expect("browser check should run");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    server.abort();
    remove_database(db_path);
}

#[test]
fn browser_check_loads_credentials_for_external_rust_server() {
    let env_file = std::env::temp_dir().join(format!(
        "codetrial-env-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(
        &env_file,
        "LIVEKIT_URL=wss://example.livekit.cloud\nLIVEKIT_API_KEY=key\nLIVEKIT_API_SECRET=secret\nGOOGLE_API_KEY=google\n",
    )
    .unwrap();

    let output = Command::new("scripts/browser-check.sh")
        .env("CODETRIAL_WEB_URL", "http://127.0.0.1:1")
        .env("CODETRIAL_CONFIG_ENV", &env_file)
        .env("BROWSER_CHECK_AGENT", "rust")
        .env("BROWSER_CHECK_VALIDATE_ENV_ONLY", "1")
        .env_remove("LIVEKIT_URL")
        .env_remove("LIVEKIT_API_KEY")
        .env_remove("LIVEKIT_API_SECRET")
        .env_remove("GOOGLE_API_KEY")
        .output()
        .expect("browser check should run");

    fs::remove_file(env_file).unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn browser_check_prints_server_log_on_node_failure() {
    let script = fs::read_to_string("scripts/browser-check.sh").unwrap();
    let driver = fs::read_to_string("scripts/browser-check.cjs").unwrap();

    assert!(script.contains("SERVER_LOG=\"$SERVER_LOG\""));
    assert!(script.contains("scripts/browser-check.cjs"));
    assert!(driver.contains("fs.readFileSync(process.env.SERVER_LOG"));
}

#[test]
fn static_interview_script_keeps_exit_fallback_short() {
    let source = fs::read_to_string("web/interview.js").unwrap();
    let ending = source_block(&source, "function endInterview", "function leaveRoom");

    // Short, but never shorter than the agent takes to answer: REPORT_TIMEOUT
    // bounds report generation at 45s and a timer-driven end spends
    // WRAP_UP_WAIT before that. Revealing the escape hatch first loses reports
    // that arrive.
    assert!(ending.contains("55000"));
    assert!(!ending.contains("25000"), "shorter than REPORT_TIMEOUT");
}

#[test]
fn static_interview_script_flushes_code_before_tests() {
    let source = fs::read_to_string("web/interview.js").unwrap();
    let run_tests = source_block(
        &source,
        "async function runTests",
        "\nfunction renderResults",
    );

    assert!(source.contains("function flushPendingCodePublish"));
    assert!(
        run_tests.find("flushPendingCodePublish()").unwrap()
            < run_tests.find("runBrowserTests").unwrap()
    );
    assert!(
        run_tests.find("flushPendingCodePublish()").unwrap()
            < run_tests.find("publish(topics.tests").unwrap()
    );
}

#[test]
fn static_interview_script_has_no_audio_unlock_overlay() {
    let source = fs::read_to_string("web/interview.js").unwrap();
    let styles = fs::read_to_string("web/styles.css").unwrap();

    assert!(!source.contains("Click to enable interviewer audio"));
    assert!(!source.contains("showAudioUnlock"));
    assert!(!styles.contains("audio-unlock"));
}

#[test]
fn static_interview_script_marks_agent_ready_visually() {
    let source = fs::read_to_string("web/interview.js").unwrap();
    let styles = fs::read_to_string("web/styles.css").unwrap();

    assert!(source.contains("function setAgentStateLabel"));
    assert!(source.contains(r#"setAgentStateLabel("Waiting", false)"#));
    assert!(
        source.contains(
            r#"setAgentStateLabel(labels[value] || providerUiState("live").label, value === "listening")"#
        )
    );
    assert!(source.contains(r#"classList.toggle("ready", ready)"#));
    assert!(styles.contains(".agent-pill.ready"));
}

#[tokio::test]
async fn token_api_matches_frontend_contract_over_http() {
    let (config, cookie, db_path) = signed_in_web_config("contract");
    let (base, server) = spawn_web_server(config).await;
    let response = reqwest::Client::new()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .json(&json!({"problemId":"merge-intervals","durationMin":120}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    let mut keys = body.as_object().unwrap().keys().collect::<Vec<_>>();
    keys.sort();
    assert_eq!(keys, ["roomName", "serverUrl", "token"]);
    assert_eq!(body["serverUrl"], "wss://example.livekit.cloud");
    assert!(body["roomName"].as_str().unwrap().starts_with("interview-"));

    let claims = claims(body["token"].as_str().unwrap());
    assert_eq!(claims["name"], "Candidate");
    assert!(claims["sub"].as_str().unwrap().starts_with("candidate-"));
    assert_eq!(
        claims["exp"].as_u64().unwrap() - claims["nbf"].as_u64().unwrap(),
        TOKEN_TTL_SECONDS
    );
    assert_eq!(claims["video"]["room"], body["roomName"]);
    assert_eq!(claims["video"]["roomJoin"], true);
    assert_eq!(claims["video"]["canPublish"], true);
    assert_eq!(claims["video"]["canSubscribe"], true);
    assert_eq!(claims["video"]["canPublishData"], true);
    let metadata: Value = serde_json::from_str(claims["metadata"].as_str().unwrap()).unwrap();
    assert_eq!(metadata["problemId"], "merge-intervals");
    assert_eq!(metadata["durationMin"], 90);
    assert_eq!(metadata["candidateIdentity"], claims["sub"]);

    server.abort();
    remove_database(db_path);
}

#[tokio::test]
async fn observer_is_not_the_candidate() {
    let (config, cookie, db_path) = signed_in_web_config("observer");
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();
    let candidate: Value = client
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let room = candidate["roomName"].as_str().unwrap();
    let candidate_claims = claims(candidate["token"].as_str().unwrap());
    let observer = client
        .post(format!("{base}/api/observer-token"))
        .header("cookie", &cookie)
        .json(&json!({ "roomName": room }))
        .send()
        .await
        .unwrap();
    assert_eq!(observer.status(), 200);
    let observer: Value = observer.json().await.unwrap();
    let observer_claims = claims(observer["token"].as_str().unwrap());
    assert_ne!(candidate_claims["sub"], observer_claims["sub"]);
    assert_eq!(
        serde_json::from_str::<Value>(candidate_claims["metadata"].as_str().unwrap()).unwrap()["candidateIdentity"],
        candidate_claims["sub"],
    );
    assert_eq!(observer_claims["video"]["canPublish"], false);
    assert_eq!(observer_claims["video"]["canPublishData"], false);
    assert!(candidate_identity_matches(
        candidate_claims["sub"].as_str().unwrap(),
        candidate_claims["metadata"].as_str().unwrap(),
    ));
    assert!(!candidate_identity_matches(
        observer_claims["sub"].as_str().unwrap(),
        "{}",
    ));

    let denied = client
        .post(format!("{base}/api/observer-token"))
        .header("cookie", &cookie)
        .json(&json!({ "roomName": "interview-not-owned" }))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 403);
    server.abort();
    remove_database(db_path);
}

#[tokio::test]
async fn token_api_rejects_malformed_body_and_defaults_an_empty_one() {
    let (mut config, cookie, db_path) = signed_in_web_config("body");
    config.fixed_room_name = Some("interview-local".to_string());
    let dispatcher = std::sync::Arc::<RecordingDispatcher>::default();
    let (base, server) =
        spawn_web_server_with_dispatcher(config, std::sync::Arc::clone(&dispatcher)).await;
    let client = reqwest::Client::new();

    // A body that is present but unparsable is a client bug. Handing back a
    // default interview would hide it until the candidate is already in the
    // room looking at the wrong problem.
    let malformed = client
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 400);
    assert!(
        dispatcher.rooms().is_empty(),
        "invalid requests must not staff a room"
    );

    // No body at all still means "give me the defaults".
    let body: Value = client
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let claims = claims(body["token"].as_str().unwrap());

    assert_eq!(body["roomName"], "interview-local");
    assert_eq!(claims["video"]["room"], "interview-local");
    let metadata: Value = serde_json::from_str(claims["metadata"].as_str().unwrap()).unwrap();
    assert_eq!(metadata["problemId"], "two-sum");
    assert_eq!(metadata["durationMin"], 45);
    assert_eq!(metadata["candidateIdentity"], claims["sub"]);
    assert_eq!(dispatcher.rooms(), vec!["interview-local"]);

    server.abort();
    remove_database(db_path);
}

#[tokio::test]
async fn token_api_ignores_empty_and_production_fixed_room() {
    let (mut empty_config, empty_cookie, empty_db_path) = signed_in_web_config("empty-room");
    empty_config.fixed_room_name = Some(String::new());
    let (empty_base, empty_server) = spawn_web_server(empty_config).await;
    let empty_body: Value = reqwest::Client::new()
        .post(format!("{empty_base}/api/token"))
        .header("cookie", &empty_cookie)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        empty_body["roomName"]
            .as_str()
            .unwrap()
            .starts_with("interview-")
    );
    empty_server.abort();
    remove_database(empty_db_path);

    let (mut production_config, production_cookie, production_db_path) =
        signed_in_web_config("production-room");
    production_config.fixed_room_name = Some("interview-local".to_string());
    production_config.production = true;
    let (production_base, production_server) = spawn_web_server(production_config).await;
    let production_body: Value = reqwest::Client::new()
        .post(format!("{production_base}/api/token"))
        .header("cookie", &production_cookie)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        production_body["roomName"]
            .as_str()
            .unwrap()
            .starts_with("interview-")
    );
    production_server.abort();
    remove_database(production_db_path);
}

/// The production hole this closes: `/api/token` invented a room name per
/// request and nothing else in the system ever learned it, so a candidate in
/// production joined a room no interviewer could be told to join and waited
/// forever. The room in the response and the room staffed must be the same
/// string, and every minted room must be staffed.
#[tokio::test]
async fn token_api_staffs_every_room_it_hands_out() {
    let (mut config, cookie, db_path) = signed_in_web_config("dispatch");
    config.production = true;
    config.fixed_room_name = None;
    let dispatcher = std::sync::Arc::<RecordingDispatcher>::default();
    let (base, server) =
        spawn_web_server_with_dispatcher(config, std::sync::Arc::clone(&dispatcher)).await;
    let client = reqwest::Client::new();

    let mut handed_out = Vec::new();
    for _ in 0..2 {
        let body: Value = client
            .post(format!("{base}/api/token"))
            .header("cookie", &cookie)
            .json(&json!({"problemId":"two-sum","durationMin":45}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        handed_out.push(body["roomName"].as_str().unwrap().to_string());
    }

    assert_eq!(dispatcher.rooms(), handed_out);
    assert_ne!(
        handed_out[0], handed_out[1],
        "production must not put two candidates in one room"
    );

    server.abort();
    remove_database(db_path);
}

/// The interviewer must be sent to the same LiveKit project whose secret signed
/// the candidate's token. Deriving that a second time from the room name, out
/// of a pool built by a second directory scan, is how the two disagree; the
/// dispatcher is handed the provider instead, and this is what says so.
#[tokio::test]
async fn a_staffed_room_names_the_project_that_signed_the_token() {
    let (mut config, cookie, db_path) = signed_in_web_config("dispatch-provider");
    config.room_prefix = "interview".to_string();
    config.fixed_room_name = None;
    config.production = true;
    config.pool = codetrial::config::ProviderPool {
        providers: vec![provider("eu", "eu")],
    };
    let dispatcher = std::sync::Arc::<RecordingDispatcher>::default();
    let (base, server) =
        spawn_web_server_with_dispatcher(config, std::sync::Arc::clone(&dispatcher)).await;

    let body: Value = reqwest::Client::new()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(body["serverUrl"], "wss://eu.livekit.cloud");
    assert_eq!(
        dispatcher.staffed(),
        vec![(
            body["roomName"].as_str().unwrap().to_string(),
            "wss://eu.livekit.cloud".to_string()
        )]
    );

    server.abort();
    remove_database(db_path);
}

/// At capacity the honest answer is no. Handing out the token anyway puts the
/// candidate alone in a room reading "Waiting", with nothing on screen or in
/// any log they can see saying why.
#[tokio::test]
async fn a_room_that_cannot_be_staffed_is_refused_rather_than_sold() {
    let (mut config, cookie, db_path) = signed_in_web_config("dispatch-capacity");
    config.production = true;
    config.fixed_room_name = None;
    let dispatcher = std::sync::Arc::<RecordingDispatcher>::default();
    dispatcher
        .at_capacity
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let (base, server) =
        spawn_web_server_with_dispatcher(config, std::sync::Arc::clone(&dispatcher)).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .json(&json!({"problemId":"two-sum","durationMin":45}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 503);
    let body: Value = response.json().await.unwrap();
    assert!(
        body["error"].as_str().unwrap().contains("Try again"),
        "the refusal must tell the candidate what to do: {body}"
    );
    assert!(body.get("token").is_none(), "a refused room has no token");

    server.abort();
    remove_database(db_path);
}

/// A web-only deployment, the other half of a split install, must keep working
/// exactly as it did: no dispatcher, no agent started here, still a token.
#[tokio::test]
async fn token_api_still_works_without_a_dispatcher() {
    let (config, cookie, db_path) = signed_in_web_config("no-dispatch");
    let (base, server) = spawn_web_server(config).await;
    let response = reqwest::Client::new()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .json(&json!({"problemId":"two-sum","durationMin":45}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 200);

    server.abort();
    remove_database(db_path);
}

#[tokio::test]
async fn token_api_rejects_oversize_body() {
    let (config, cookie, db_path) = signed_in_web_config("oversize");
    let dispatcher = std::sync::Arc::<RecordingDispatcher>::default();
    let (base, server) =
        spawn_web_server_with_dispatcher(config, std::sync::Arc::clone(&dispatcher)).await;
    let response = reqwest::Client::new()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .body(vec![b'a'; MAX_BODY_BYTES + 1])
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 413);

    // The other half of the refusal: a body too large to read is settled before
    // an interviewer is sent anywhere, so the room this request never got is
    // not left staffed for the length of an interview nobody is in.
    assert!(
        dispatcher.rooms().is_empty(),
        "an oversize request must not staff a room"
    );

    server.abort();
    remove_database(db_path);
}

#[tokio::test]
async fn token_api_fails_closed_with_frontend_error_shape_without_livekit_credentials() {
    let (mut config, cookie, db_path) = signed_in_web_config("missing-livekit");
    config.pool = Default::default();
    let (base, server) = spawn_web_server(config).await;
    let response = reqwest::Client::new()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .json(&json!({"problemId":"two-sum","durationMin":45}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 500);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["error"],
        "Server is missing LiveKit credentials. Create config/codetrial.env.local or set LIVEKIT_URL, LIVEKIT_API_KEY and LIVEKIT_API_SECRET."
    );

    server.abort();
    remove_database(db_path);
}

#[test]
fn account_recording_config_does_not_require_github_oauth() {
    let mut config = web_config();

    assert!(login_config(&config).is_none());

    config.session_secret = Some("session".to_string());
    config.db_path = Some(Path::new("codetrial.db").to_path_buf());

    let login = login_config(&config).expect("recording config should enable account sessions");

    // `is_none` rather than `assert_eq!(.., None)`: that form needs `Debug` on
    // `GitHubOauth`, and the client secret is the reason that type does not
    // have one.
    assert!(
        login.oauth.is_none(),
        "no OAuth app means no credentials, not blank ones"
    );
    assert_eq!(login.session_secret, "session");
    assert_eq!(login.db_path, Path::new("codetrial.db"));
}

#[test]
fn account_database_schema_creates_login_tables() {
    let path = std::env::temp_dir().join(format!(
        "codetrial-accounts-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    initialize_account_database(&path).unwrap();

    let connection = rusqlite::Connection::open(&path).unwrap();
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap();
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(tables.contains(&"users".to_string()));
    assert!(tables.contains(&"sessions".to_string()));
    assert!(tables.contains(&"reports".to_string()));

    for (table, expected) in [
        (
            "users",
            [
                "github_id INTEGER NOT NULL UNIQUE",
                "login TEXT NOT NULL",
                "created_at INTEGER NOT NULL",
                "updated_at INTEGER NOT NULL",
            ]
            .as_slice(),
        ),
        (
            "sessions",
            [
                "user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE",
                "expires_at INTEGER NOT NULL",
                "created_at INTEGER NOT NULL",
            ]
            .as_slice(),
        ),
        (
            "reports",
            [
                "user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE",
                "problem_id TEXT NOT NULL",
                "payload TEXT NOT NULL",
                "created_at INTEGER NOT NULL",
                "updated_at INTEGER NOT NULL",
            ]
            .as_slice(),
        ),
    ] {
        let sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        for snippet in expected {
            assert!(
                sql.contains(snippet),
                "{table} schema should contain {snippet}, got {sql}"
            );
        }
    }

    remove_database(path);
}

#[tokio::test]
async fn account_routes_record_interviewee_github_login() {
    let (base, server, path, client) = account_server("record-login").await;

    let login = client
        .get(format!("{base}/api/login"))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 400);
    assert_eq!(
        login.json::<Value>().await.unwrap()["error"],
        "Submit a GitHub username to start an interview."
    );

    let invalid = client
        .post(format!("{base}/api/login"))
        .json(&json!({"login":"-bad-"}))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), 400);

    let session_cookie = record_login(&client, &base, "@Octo-Cat").await;

    // No OAuth app is configured, so there is no code to exchange and the
    // callback must say so instead of posting empty credentials to GitHub.
    let callback = client
        .get(format!("{base}/api/callback?code=x&state=y"))
        .send()
        .await
        .unwrap();
    assert_eq!(callback.status(), 400);

    let session = client
        .get(format!("{base}/api/session"))
        .send()
        .await
        .unwrap();
    assert_eq!(session.status(), 200);

    assert_eq!(
        session.json::<Value>().await.unwrap(),
        json!({"signedIn": false, "loginRequired": true, "maxDurationMin": MAX_DURATION_MIN})
    );

    let signed_in = client
        .get(format!("{base}/api/session"))
        .header("cookie", session_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(signed_in["signedIn"], true);
    assert_eq!(signed_in["user"]["login"], "octo-cat");

    let reports = client
        .get(format!("{base}/api/reports"))
        .send()
        .await
        .unwrap();
    assert_eq!(reports.status(), 401);
    let save_report = client
        .post(format!("{base}/api/reports"))
        .json(&json!({"problemId":"two-sum"}))
        .send()
        .await
        .unwrap();
    assert_eq!(save_report.status(), 401);

    server.abort();
    remove_database(path);
}

/// A typed handle is not proof of anything, so it must not be the key an
/// account is found by. If it were, typing a name someone else used would hand
/// over their interview history, and nothing stops anyone typing any name.
#[tokio::test]
async fn a_claimed_handle_does_not_reach_an_earlier_candidates_reports() {
    let (base, server, path, client) = account_server("claimed-handle").await;

    let first = record_login(&client, &base, "octocat").await;
    let saved = client
        .post(format!("{base}/api/reports"))
        .header("cookie", &first)
        .json(&json!({"id":"report-one","problemId":"two-sum","score":8}))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);

    let second = record_login(&client, &base, "octocat").await;
    assert_ne!(first, second, "each recorded login is its own account");

    let reports = client
        .get(format!("{base}/api/reports"))
        .header("cookie", &second)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        reports["reports"].as_array().unwrap().len(),
        0,
        "claiming a handle must not inherit the reports filed under it"
    );

    let mine = client
        .get(format!("{base}/api/reports"))
        .header("cookie", &first)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(mine["reports"][0]["id"], "report-one");

    server.abort();
    remove_database(path);
}

/// The other half of the fail-closed rule, and the one an operator actually
/// reaches: the file opens fine, but a rolled-back binary finds a schema
/// version from the future and refuses to migrate it. That arm has to answer
/// the same way as an unopenable path, because "I cannot read this database" is
/// not the same statement as "this server has no accounts".
#[tokio::test]
async fn an_unmigratable_account_database_refuses_rather_than_disabling_login() {
    let path = account_db_path("unmigratable");
    initialize_account_database(&path).unwrap();
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(&format!(
            "PRAGMA user_version = {}",
            codetrial::web::ACCOUNT_SCHEMA_VERSION + 1
        ))
        .unwrap();
    let mut config = web_config();
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();

    let session = client
        .get(format!("{base}/api/session"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        session,
        json!({"signedIn": false, "loginRequired": true, "maxDurationMin": MAX_DURATION_MIN}),
        "a database this binary cannot migrate still requires a login"
    );

    let token = client
        .post(format!("{base}/api/token"))
        .json(&json!({"problemId":"two-sum","durationMin":45}))
        .send()
        .await
        .unwrap();
    assert_eq!(token.status(), 503);

    server.abort();
    remove_database(path);
}

/// A database that will not open must not read as "this server has no
/// accounts". That would drop the sign-in gate over a bad path: the browser
/// would be told login is optional and let anyone start an interview.
#[tokio::test]
async fn an_unopenable_account_database_refuses_rather_than_disabling_login() {
    // A directory is a path SQLite cannot open as a database file.
    let path = account_db_path("unopenable");
    fs::create_dir_all(&path).unwrap();
    let mut config = web_config();
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();

    let session = client
        .get(format!("{base}/api/session"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        session,
        json!({"signedIn": false, "loginRequired": true, "maxDurationMin": MAX_DURATION_MIN}),
        "a broken database still requires a login; it cannot serve one"
    );

    let token = client
        .post(format!("{base}/api/token"))
        .json(&json!({"problemId":"two-sum","durationMin":45}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        token.status(),
        503,
        "unavailable, not 404: the operator has a broken database, not an unconfigured one"
    );

    let login = client
        .post(format!("{base}/api/login"))
        .json(&json!({"login":"octocat"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 503);

    server.abort();
    fs::remove_dir_all(path).unwrap();
}

/// `/api/token` needs a session, and this is where sessions come from, so an
/// unlimited login endpoint just moves the flood one door back and grows the
/// account database while it is at it.
#[tokio::test]
async fn login_endpoint_rate_limits_a_noisy_client() {
    let (base, server, path, client) = account_server("login-rate-limit").await;
    let url = format!("{base}/api/login");

    for attempt in 1..=TOKEN_RATE_LIMIT {
        let response = client
            .post(&url)
            .json(&json!({"login":"octocat"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "attempt {attempt} should pass");
    }

    let blocked = client
        .post(&url)
        .json(&json!({"login":"octocat"}))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 429);
    assert_eq!(blocked.headers().get("retry-after").unwrap(), "60");

    server.abort();
    remove_database(path);
}

#[tokio::test]
async fn login_rejects_a_body_that_is_not_json() {
    let (base, server, path, client) = account_server("login-malformed").await;

    let malformed = client
        .post(format!("{base}/api/login"))
        .header("content-type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();
    assert_eq!(malformed.status(), 400);

    let oversize = client
        .post(format!("{base}/api/login"))
        .body(vec![b'a'; MAX_BODY_BYTES + 1])
        .send()
        .await
        .unwrap();
    assert_eq!(oversize.status(), 413);

    for handle in ["", "-bad-", "bad name", &"a".repeat(40)] {
        let rejected = client
            .post(format!("{base}/api/login"))
            .json(&json!({ "login": handle }))
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), 400, "{handle:?} is not a GitHub handle");
    }

    server.abort();
    remove_database(path);
}

#[tokio::test]
async fn github_callback_sets_session_and_clears_oauth_state() {
    let (github_base, github_server) = spawn_mock_github().await;
    let path = std::env::temp_dir().join(format!(
        "codetrial-callback-accounts-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    config.github_oauth_base_url = Some(github_base.clone());
    config.github_api_base_url = Some(github_base);

    // The schema is created once at startup, not per request, so a caller that
    // builds the router directly has to do what `main` does.
    initialize_account_database(&path).unwrap();
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let response = client
        .get(format!("{base}/api/callback?code=ok&state=state-token"))
        .header(
            "cookie",
            format!(
                "codetrial_oauth_state={}",
                signed_cookie("state-token", "session-secret")
            ),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 302);
    let cookies = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(cookies.len(), 2);
    let session_cookie = cookies
        .iter()
        .find(|cookie| cookie.starts_with("codetrial_session="))
        .expect("callback should set session cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert!(
        cookies
            .iter()
            .any(|cookie| cookie.starts_with("codetrial_oauth_state=; Max-Age=0")),
        "callback should clear oauth state cookie"
    );

    let session = reqwest::Client::new()
        .get(format!("{base}/api/session"))
        .header("cookie", session_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(session["signedIn"], true);
    assert_eq!(session["user"]["login"], "octocat");

    server.abort();
    github_server.abort();
    remove_database(path);
}

/// Hiding the start button proves nothing: /interview is a URL anyone can open,
/// and the token is a live LiveKit credential. Where accounts exist, the
/// credential is what has to refuse.
#[tokio::test]
async fn token_requires_a_session_once_accounts_exist() {
    let path = std::env::temp_dir().join(format!(
        "codetrial-token-gate-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    initialize_account_database(&path).unwrap();
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (1, 101, 'one', 1, 1)",
                [],
            )
            .unwrap();
        insert_session(&connection, "session-one", 1);
    }

    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    let (base, server) = spawn_web_server(config).await;
    let url = format!("{base}/api/token");
    let body = json!({"problemId": "two-sum", "durationMin": 45});

    let anonymous = reqwest::Client::new()
        .post(&url)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous.status(), 401);
    assert!(
        anonymous.json::<Value>().await.unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("GitHub username"),
        "the refusal has to say what to do about it"
    );

    let signed_in = reqwest::Client::new()
        .post(&url)
        .header(
            "cookie",
            format!(
                "codetrial_session={}",
                signed_cookie("session-one", "session-secret")
            ),
        )
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(signed_in.status(), 200);
    assert!(signed_in.json::<Value>().await.unwrap()["token"].is_string());

    server.abort();
    remove_database(path);
}

/// The rate limit is keyed by address. If anonymous callers could spend it, a
/// stranger on the same NAT could lock the signed-in candidate out of their own
/// interview without ever holding a session.
#[tokio::test]
async fn anonymous_requests_cannot_spend_the_signed_in_budget() {
    let path = std::env::temp_dir().join(format!(
        "codetrial-token-budget-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    initialize_account_database(&path).unwrap();
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (1, 101, 'one', 1, 1)",
                [],
            )
            .unwrap();
        insert_session(&connection, "session-one", 1);
    }

    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    let (base, server) = spawn_web_server(config).await;
    let url = format!("{base}/api/token");
    let client = reqwest::Client::new();

    // Twice the whole budget, all of it unauthenticated.
    for _ in 0..(TOKEN_RATE_LIMIT * 2) {
        let refused = client.post(&url).json(&json!({})).send().await.unwrap();
        assert_eq!(refused.status(), 401);
    }

    let signed_in = client
        .post(&url)
        .header(
            "cookie",
            format!(
                "codetrial_session={}",
                signed_cookie("session-one", "session-secret")
            ),
        )
        .json(&json!({}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        signed_in.status(),
        200,
        "the candidate's budget was spent by people who never signed in"
    );

    server.abort();
    remove_database(path);
}

#[test]
fn github_oauth_config_is_optional_for_recorded_login() {
    let mut config = web_config();
    assert!(login_config(&config).is_none());

    config.github_client_id = Some("client".to_string());
    config.db_path = Some(Path::new("accounts.db").to_path_buf());
    assert!(
        login_config(&config).is_none(),
        "a database with no cookie secret cannot hold a session"
    );

    config.session_secret = Some("secret".to_string());
    assert!(
        login_config(&config).is_some(),
        "a cookie secret and a database are all a recorded login needs"
    );
}

/// Interview LiveKit credentials always belong to a signed-in GitHub user.
#[tokio::test]
async fn token_requires_recorded_github_login() {
    let path = std::env::temp_dir().join(format!(
        "codetrial-token-record-login-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    initialize_account_database(&path).unwrap();
    let mut config = web_config();
    config.session_secret = Some("secret".to_string());
    config.db_path = Some(path.clone());
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();

    let anonymous = client
        .post(format!("{base}/api/token"))
        .json(&json!({"problemId": "two-sum", "durationMin": 45}))
        .send()
        .await
        .unwrap();

    assert_eq!(anonymous.status(), 401);

    let session_cookie = record_login(&client, &base, "octocat").await;
    let signed_in = client
        .post(format!("{base}/api/token"))
        .header("cookie", session_cookie)
        .json(&json!({"problemId": "two-sum", "durationMin": 45}))
        .send()
        .await
        .unwrap();

    assert_eq!(signed_in.status(), 200);
    server.abort();
    remove_database(path);
}

#[tokio::test]
async fn account_reports_are_scoped_to_the_signed_in_user() {
    let path = std::env::temp_dir().join(format!(
        "codetrial-route-accounts-{}-{}.db",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    initialize_account_database(&path).unwrap();
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection.execute(
            "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (1, 101, 'one', 1, 1)",
            [],
        ).unwrap();
        connection.execute(
            "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (2, 202, 'two', 1, 1)",
            [],
        ).unwrap();
        insert_session(&connection, "session-one", 1);
        insert_session(&connection, "session-two", 2);
        connection.execute(
            "INSERT INTO reports (id, user_id, problem_id, payload, created_at, updated_at) VALUES ('report-one', 1, 'two-sum', '{\"problemId\":\"two-sum\",\"score\":8}', 1, 1)",
            [],
        ).unwrap();
        connection.execute(
            "INSERT INTO reports (id, user_id, problem_id, payload, created_at, updated_at) VALUES ('report-two', 2, 'merge-intervals', '{\"problemId\":\"merge-intervals\",\"score\":4}', 1, 1)",
            [],
        ).unwrap();
    }

    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();
    let user_one_cookie = format!(
        "codetrial_session={}",
        signed_cookie("session-one", "session-secret")
    );
    let user_two_cookie = format!(
        "codetrial_session={}",
        signed_cookie("session-two", "session-secret")
    );

    let session = client
        .get(format!("{base}/api/session"))
        .header("cookie", &user_one_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(session["signedIn"], true);
    assert_eq!(session["user"]["login"], "one");

    let reports = client
        .get(format!("{base}/api/reports"))
        .header("cookie", &user_one_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(reports["reports"].as_array().unwrap().len(), 1);
    assert_eq!(reports["reports"][0]["id"], "report-one");

    let saved = client
        .post(format!("{base}/api/reports"))
        .header("cookie", &user_one_cookie)
        .json(&json!({"id":"new-report","problemId":"three-sum","score":9}))
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), 200);
    assert_eq!(saved.json::<Value>().await.unwrap()["id"], "new-report");

    let blocked = client
        .post(format!("{base}/api/reports"))
        .header("cookie", &user_two_cookie)
        .json(&json!({"id":"new-report","problemId":"three-sum","score":1}))
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 409);

    let user_two_reports = client
        .get(format!("{base}/api/reports"))
        .header("cookie", &user_two_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let ids = user_two_reports["reports"]
        .as_array()
        .unwrap()
        .iter()
        .map(|report| report["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["report-two"]);

    let logout = client
        .post(format!("{base}/api/logout"))
        .header("cookie", &user_one_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 200);
    let signed_out = client
        .get(format!("{base}/api/session"))
        .header("cookie", &user_one_cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        signed_out,
        json!({"signedIn": false, "loginRequired": true, "maxDurationMin": MAX_DURATION_MIN}),
        "accounts exist here, so the browser has to know to demand a sign-in"
    );

    server.abort();
    remove_database(path);
}

/// `/api/reports` is an authenticated write with no rate limit, and the client
/// picks its own row ids, so without a ceiling one account can grow the
/// database until the disk runs out. 507 rather than 429: waiting does not
/// help, so telling the client to retry would be a lie.
#[tokio::test]
async fn a_full_account_cannot_grow_the_report_database() {
    let (config, cookie, path) = signed_in_web_config("report-quota");
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        for index in 0..MAX_REPORTS_PER_USER {
            connection.execute(
                "INSERT INTO reports (id, user_id, problem_id, payload, created_at, updated_at) VALUES (?1, 1, 'two-sum', '{}', 1, 1)",
                [format!("seeded-{index}")],
            ).unwrap();
        }
    }
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();

    let refused = client
        .post(format!("{base}/api/reports"))
        .header("cookie", &cookie)
        .json(&json!({"id":"one-too-many","problemId":"three-sum","score":9}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 507);

    // The interview in progress still has to be able to save, or the ceiling
    // breaks the feature it protects.
    let rewritten = client
        .post(format!("{base}/api/reports"))
        .header("cookie", &cookie)
        .json(&json!({"id":"seeded-0","problemId":"two-sum","score":10}))
        .send()
        .await
        .unwrap();
    assert_eq!(rewritten.status(), 200);

    let stored = client
        .get(format!("{base}/api/reports"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(
        stored["reports"].as_array().unwrap().len() as i64,
        MAX_REPORTS_PER_USER,
        "the refused write must not have landed",
    );

    server.abort();
    remove_database(path);
}

#[test]
fn body_limit_matches_non_working_reference() {
    assert_eq!(MAX_BODY_BYTES, 8192);

    // Reports carry graded prose, so they get their own, larger ceiling; the
    // browser's per-field caps in web/lib.js are sized against this one.
    assert_eq!(MAX_REPORT_BYTES, 65536);
    assert_eq!(DEFAULT_WEB_DIR, "web");
}

/// A pool holding just the primary, which is what a single-project deployment
/// has. The credentials used to sit in three flat `WebServerConfig` fields
/// beside the pool; the pool is now the only place they live.
fn primary_pool(url: &str, api_key: &str, api_secret: &str) -> codetrial::config::ProviderPool {
    codetrial::config::ProviderPool {
        providers: vec![codetrial::config::Provider {
            id: codetrial::config::PRIMARY_PROVIDER_ID.to_string(),
            url: url.to_string(),
            api_key: api_key.to_string(),
            api_secret: api_secret.to_string(),
            google_api_key: String::new(),
        }],
    }
}

fn provider(id: &str, host: &str) -> codetrial::config::Provider {
    codetrial::config::Provider {
        id: id.to_string(),
        url: format!("wss://{host}.livekit.cloud"),
        api_key: format!("{id}-key"),
        api_secret: format!("{id}-secret"),
        google_api_key: format!("{id}-google"),
    }
}

/// The property that matters is not that the web process picks a provider, it
/// is
/// that the agent process, which only ever receives the room name, resolves the
/// same one. Asserting that by calling the selector the handler itself uses
/// would pass for any implementation that is merely self-consistent, so this
/// walks the path the agent walks: read the id out of the room name, look it
/// up,
/// and check the token was signed with that provider's secret.
#[tokio::test]
async fn a_minted_room_name_routes_the_agent_to_the_provider_that_signed_it() {
    let (mut config, cookie, db_path) = signed_in_web_config("provider-routing");
    config.room_prefix = "interview".to_string();
    config.fixed_room_name = None;
    config.pool = codetrial::config::ProviderPool {
        providers: vec![
            provider(codetrial::config::PRIMARY_PROVIDER_ID, "primary"),
            provider("eu", "eu"),
            provider("us", "us"),
        ],
    };
    let pool = config.pool.clone();
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();

    let mut served = Vec::new();
    for _ in 0..6 {
        let response = client
            .post(format!("{base}/api/token"))
            .header("cookie", &cookie)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body = response.json::<Value>().await.unwrap();
        let room_name = body["roomName"].as_str().unwrap().to_string();

        // The same call the agent makes, given nothing but the room name.
        let expected = pool.for_room(&room_name, "interview").unwrap();

        assert_eq!(body["serverUrl"], expected.url);

        // A URL from one project and a signature from another would still look
        // right in the response body and fail at the SFU.
        let token = body["token"].as_str().unwrap();
        assert_eq!(claims(token)["iss"], expected.api_key);
        assert!(verify_signature(token, &expected.api_secret));
        served.push(expected.id.clone());
    }

    // Round robin, so six requests over three providers use all three. The old
    // room-name hash could send every room to the same one.
    served.sort();
    served.dedup();
    assert_eq!(served, vec!["eu", "primary", "us"]);

    server.abort();
    remove_database(db_path);
}

/// A fixed room name is handed to the agent verbatim, so the handler has to
/// read
/// the provider out of it rather than take the next one off the counter.
#[tokio::test]
async fn a_fixed_room_name_pins_the_provider_it_names() {
    let (mut config, cookie, db_path) = signed_in_web_config("provider-fixed-room");
    config.room_prefix = "interview".to_string();
    config.fixed_room_name = Some("interview-eu-fixed".to_string());
    config.production = false;
    config.pool = codetrial::config::ProviderPool {
        providers: vec![
            provider(codetrial::config::PRIMARY_PROVIDER_ID, "primary"),
            provider("eu", "eu"),
        ],
    };
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();

    for _ in 0..3 {
        let body = client
            .post(format!("{base}/api/token"))
            .header("cookie", &cookie)
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        assert_eq!(body["roomName"], "interview-eu-fixed");
        assert_eq!(body["serverUrl"], "wss://eu.livekit.cloud");
    }

    server.abort();
    remove_database(db_path);
}

fn web_config() -> WebServerConfig {
    WebServerConfig {
        web_dir: std::env::temp_dir(),
        github_client_id: None,
        github_client_secret: None,
        session_secret: None,
        db_path: None,
        github_oauth_base_url: None,
        github_api_base_url: None,
        room_prefix: "interview".to_string(),
        fixed_room_name: None,
        production: false,
        compiler_explorer_enabled: true,
        trusted_proxy_hops: 0,
        recording: None,
        pool: primary_pool("wss://example.livekit.cloud", "devkey", "devsecret"),
        probe_provider_quota: false,
    }
}

/// Writes the row `create_session` would have written for `token`. These
/// fixtures pin explicit user ids, so they insert directly instead of signing
/// in, but the id column still holds the digest rather than the cookie value.
fn insert_session(connection: &rusqlite::Connection, token: &str, user_id: i64) {
    connection
        .execute(
            "INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES (?1, ?2, 9999999999, 1)",
            (codetrial::accounts::session_key(token), user_id),
        )
        .unwrap();
}

fn signed_in_web_config(label: &str) -> (WebServerConfig, String, std::path::PathBuf) {
    let db_path = account_db_path(label);
    initialize_account_database(&db_path).unwrap();
    {
        let connection = rusqlite::Connection::open(&db_path).unwrap();
        connection
            .execute(
                "INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (1, 101, 'one', 1, 1)",
                [],
            )
            .unwrap();
        insert_session(&connection, "session-one", 1);
    }

    let mut config = web_config();
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(db_path.clone());
    (
        config,
        format!(
            "codetrial_session={}",
            signed_cookie("session-one", "session-secret")
        ),
        db_path,
    )
}

/// A SQLite database in WAL mode is three files, and removing only the first
/// left the other two behind on every run: the system temp directory had
/// collected 2657 of them. `src/accounts.rs` has always cleaned all three for
/// its own scratch databases; this side never learned to.
///
/// The main file keeps its `unwrap`, so a test that was asserting the database
/// existed still does. The sidecars do not: WAL leaves them behind only if the
/// database was actually opened, and a test that never opened it is not broken.
fn remove_database(path: impl AsRef<std::path::Path>) {
    let path = path.as_ref();
    fs::remove_file(path).unwrap();
    for suffix in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{suffix}", path.display()));
    }
}

fn account_db_path(label: &str) -> std::path::PathBuf {
    unique_temp_path(label, ".db")
}

/// A server with accounts enabled and nobody signed in yet, which is where
/// every login test starts.
async fn account_server(
    label: &str,
) -> (
    String,
    tokio::task::JoinHandle<Result<(), std::io::Error>>,
    std::path::PathBuf,
    reqwest::Client,
) {
    let db_path = account_db_path(label);
    initialize_account_database(&db_path).unwrap();
    let mut config = web_config();
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(db_path.clone());
    let (base, server) = spawn_web_server(config).await;
    (base, server, db_path, reqwest::Client::new())
}

/// Signs in the way the browser does, so a test that cares about who owns what
/// does not have to hand-write rows and cookie signatures.
async fn record_login(client: &reqwest::Client, base: &str, handle: &str) -> String {
    let response = client
        .post(format!("{base}/api/login"))
        .json(&json!({ "login": handle }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "recording {handle} should succeed");
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|cookie| cookie.starts_with("codetrial_session="))
        .expect("login should set a session cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

/// A LiveKit token names its project in `iss` and proves it in the signature.
/// Checking only `iss` would miss a token minted with one provider's key and
/// another's secret, which the SFU rejects and the response body does not show.
fn verify_signature(token: &str, secret: &str) -> bool {
    // A JWS is `<signed>.<signature>`, the same shape `signed_cookie` builds,
    // so re-signing and comparing beats a second HMAC spelled out here.
    let (signed, _) = token.rsplit_once('.').unwrap();
    signed_cookie(signed, secret) == token
}

fn signed_cookie(value: &str, secret: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(value.as_bytes());
    format!(
        "{value}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

async fn spawn_web_server(
    config: WebServerConfig,
) -> (String, tokio::task::JoinHandle<Result<(), std::io::Error>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server =
        tokio::spawn(
            async move { axum::serve(listener, codetrial::web::web_service(config)).await },
        );
    (format!("http://{addr}"), server)
}

/// Records what it was asked to staff, so a test can assert that the room a
/// candidate is handed is the room an interviewer was sent to, on the LiveKit
/// project whose secret signed that candidate's token.
#[derive(Default)]
struct RecordingDispatcher {
    staffed: std::sync::Mutex<Vec<(String, String)>>,
    /// Set to refuse, standing in for a process already at capacity.
    at_capacity: std::sync::atomic::AtomicBool,
}

impl RoomDispatcher for RecordingDispatcher {
    fn ensure_agent(&self, room_name: &str, provider: &codetrial::config::Provider) -> bool {
        if self.at_capacity.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        self.staffed
            .lock()
            .unwrap()
            .push((room_name.to_string(), provider.url.clone()));
        true
    }
}

impl RecordingDispatcher {
    fn rooms(&self) -> Vec<String> {
        self.staffed
            .lock()
            .unwrap()
            .iter()
            .map(|(room_name, _)| room_name.clone())
            .collect()
    }

    fn staffed(&self) -> Vec<(String, String)> {
        self.staffed.lock().unwrap().clone()
    }
}

async fn spawn_web_server_with_dispatcher(
    config: WebServerConfig,
    dispatcher: std::sync::Arc<RecordingDispatcher>,
) -> (String, tokio::task::JoinHandle<Result<(), std::io::Error>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            codetrial::web::web_service_with_dispatcher(config, Some(dispatcher)),
        )
        .await
    });
    (format!("http://{addr}"), server)
}

async fn spawn_mock_github() -> (String, tokio::task::JoinHandle<Result<(), std::io::Error>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = axum::Router::new()
        .route(
            "/login/oauth/access_token",
            axum::routing::post(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    json!({"access_token":"mock-token"}).to_string(),
                )
            }),
        )
        .route(
            "/user",
            axum::routing::get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    json!({"id":303,"login":"octocat","avatar_url":"https://example.test/avatar.png"}).to_string(),
                )
            }),
        )

        // What `read:user` alone cannot answer. The unverified and secondary
        // entries are here because selecting the wrong one is the failure this
        // endpoint exists to make possible.
        .route(
            "/user/emails",
            axum::routing::get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    json!([
                        {"email":"unverified@example.test","primary":false,"verified":false},
                        {"email":"secondary@example.test","primary":false,"verified":true},
                        {"email":"octocat@example.test","primary":true,"verified":true}
                    ])
                    .to_string(),
                )
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    (format!("http://{addr}"), server)
}

/// The bank as the browser will see it, assembled from the per-problem files
/// under `web/` rather than from `problem-bank/`.
///
/// Reading the generated side on purpose. `gen-problems.py --check` already
/// proves the two agree, and reading the source here would make these tests
/// pass on a tree where the files the browser actually fetches were never
/// regenerated.
fn browser_problem_bank() -> Value {
    let mut problems: Vec<Value> = read_json_dir("web/problems").into_values().collect();
    problems.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    Value::Array(problems)
}

fn browser_judges() -> Value {
    Value::Object(read_json_dir("web/judges").into_iter().collect())
}

fn read_json_dir(path: &str) -> std::collections::BTreeMap<String, Value> {
    fs::read_dir(path)
        .unwrap_or_else(|error| panic!("{path} should be generated: {error}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| {
            let id = path.file_stem().unwrap().to_str().unwrap().to_string();
            let text = fs::read_to_string(&path).unwrap();
            let value = serde_json::from_str(&text)
                .unwrap_or_else(|error| panic!("{} should parse: {error}", path.display()));
            (id, value)
        })
        .collect()
}

fn source_block<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_index = source
        .find(start)
        .unwrap_or_else(|| panic!("missing {start}"));
    let end_index = source[start_index..]
        .find(end)
        .map(|index| start_index + index)
        .unwrap_or_else(|| panic!("missing {end}"));
    &source[start_index..end_index]
}

/// The top-level arguments of the first `name(` call in `source`, so a nested
/// call's own commas do not split the list. `clamp(parseInt(x, 10) || 45, 10,
/// 90)` has three arguments, not four.
fn call_arguments<'a>(source: &'a str, name: &str) -> Vec<&'a str> {
    let start = source
        .find(name)
        .unwrap_or_else(|| panic!("missing {name} in the browser source"))
        + name.len();
    let mut depth = 1usize;
    let mut arguments = Vec::new();
    let mut argument_start = start;

    // A paren or comma inside a string literal is text, not structure. Today's
    // call has `params.get("duration")` with neither, so this changes nothing;
    // it is here so that adding one later mis-parses nothing rather than
    // shifting the bounds this test is supposed to be pinning.
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (offset, character) in source[start..].char_indices() {
        let at = start + offset;
        if let Some(open) = quote {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == open {
                quote = None;
            }
            continue;
        }
        match character {
            '"' | '\'' | '`' => quote = Some(character),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    arguments.push(source[argument_start..at].trim());
                    break;
                }
            }
            ',' if depth == 1 => {
                arguments.push(source[argument_start..at].trim());
                argument_start = at + 1;
            }
            _ => {}
        }
    }
    arguments
}

fn browser_number(source: &str, after: &str, until: char) -> u32 {
    let start = source
        .find(after)
        .unwrap_or_else(|| panic!("missing {after} in the browser source"))
        + after.len();
    let rest = &source[start..];
    let end = rest
        .find(until)
        .unwrap_or_else(|| panic!("{after} is not terminated by {until}"));
    rest[..end]
        .trim()
        .parse()
        .unwrap_or_else(|error| panic!("{after} is not a number: {error}"))
}

/// `src/config.rs` owns the interview length and says the browser lobby mirrors
/// the range, which nothing checked. The two are not interchangeable: the
/// browser countdown and `run_room`'s server-side hard deadline are both built
/// from this number, so a lobby offering a length the server clamps gives the
/// candidate a timer that disagrees with the process that will actually end
/// their interview.
#[test]
fn browser_interview_duration_matches_the_server_clamp() {
    let interview = fs::read_to_string("web/interview.js").unwrap();
    let page = fs::read_to_string("web/index.html").unwrap();

    let clamp = call_arguments(
        &interview[interview.find("const durationMin = ").unwrap()..],
        "clamp(",
    );
    assert_eq!(
        clamp.len(),
        3,
        "clamp takes a value and two bounds: {clamp:?}"
    );

    let parsed = |value: &str| -> u32 {
        value
            .parse()
            .unwrap_or_else(|error| panic!("{value} is not a bound: {error}"))
    };
    assert_eq!(
        parsed(clamp[1]),
        MIN_DURATION_MIN,
        "web/interview.js clamps below the server minimum"
    );
    assert_eq!(
        parsed(clamp[2]),
        MAX_DURATION_MIN,
        "web/interview.js clamps above the server maximum"
    );

    // The fallback the query string gets when it carries no duration.
    let fallback = clamp[0]
        .rsplit("||")
        .next()
        .expect("the clamped value falls back to a literal")
        .trim();
    assert_eq!(
        parsed(fallback),
        DEFAULT_DURATION_MIN,
        "web/interview.js falls back to a different default than the server"
    );

    // Every button the lobby offers has to be a length the server will honour,
    // or the candidate picks one number and is given another without being
    // told.
    let mut offered = Vec::new();
    for chunk in page.split("data-duration=\"").skip(1) {
        let value = chunk.split('"').next().expect("data-duration is quoted");
        let minutes: u32 = value
            .parse()
            .unwrap_or_else(|error| panic!("data-duration={value} is not a number: {error}"));
        assert!(
            (MIN_DURATION_MIN..=MAX_DURATION_MIN).contains(&minutes),
            "web/index.html offers {minutes} minutes, which the server clamps to \
             {MIN_DURATION_MIN}..={MAX_DURATION_MIN}"
        );
        offered.push(minutes);
    }
    assert!(!offered.is_empty(), "the lobby offers no durations at all");

    // The preselected button decides what a candidate who touches nothing gets.
    let selected = page
        .split("duration-button selected\"")
        .nth(1)
        .expect("one duration button is preselected");
    assert_eq!(
        browser_number(selected, "data-duration=\"", '"'),
        DEFAULT_DURATION_MIN,
        "the preselected lobby duration is not the server default"
    );

    // The lobby derives its length from the checked difficulty, so the two
    // assertions above are only worth anything if exactly one box starts
    // checked: two would make the derived length ambiguous, and none would
    // leave `suggestedDuration` deciding from an empty set.
    //
    // What that box derives is checked in the browser rather than here, by "a
    // candidate who touches nothing gets the server's own default length" in
    // tests/browser/lobby.test.js, which asserts that the length app.js renders
    // on an untouched lobby is the preselected one this test just pinned to
    // DEFAULT_DURATION_MIN. There is no longer a lobby-side literal to read:
    // app.js derives the length, so the constant reaches the candidate through
    // the markup. Restating the mapping here in Rust would be a second copy
    // that drifts: swapping which difficulty yields thirty keeps every number
    // inside the cap, so a hand-written mirror of the mapping went on passing
    // while the untouched default had moved.
    //
    // Each `<input>` start tag is read whole so the check does not care what
    // order its attributes are written in.
    let checked = page
        .split("<input")
        .skip(1)
        .map(|tag| &tag[..tag.find('>').unwrap_or(tag.len())])
        .filter(|tag| tag.contains("name=\"difficulty\"") && tag.contains("checked"))
        .count();
    assert_eq!(
        checked, 1,
        "exactly one difficulty starts checked, or the length it derives is ambiguous"
    );

    // The other half of this, that a suggested length is one a default
    // deployment can record to the end, is a `const` assertion beside the two
    // constants in src/config.rs and a rendered-attribute check in
    // tests/browser/lobby.test.js. Neither belongs here: one is a compile-time
    // relation and the other is what a browser paints.
}

/// A card the filter cannot show is a problem nobody can reach.
///
/// web/app.js hides every card whose `data-difficulty` is not among the checked
/// boxes, so a generated card carrying a level the fieldset never offers is
/// invisible for good, is never recommended, and nothing fails to say so. The
/// cards come from problem-bank/problems.json through
/// scripts/gen-problem-cards.py and the boxes are hand-written, which is
/// exactly the seam a new difficulty string would slip through.
///
/// Both sides are read off the rendered page rather than restated here, because
/// a third copy of the level names is a third place to forget one.
#[test]
fn browser_every_problem_card_has_a_difficulty_the_filter_offers() {
    let page = fs::read_to_string("web/index.html").unwrap();

    // Each `<input>` start tag read whole, so the check does not care what
    // order its attributes are written in.
    let offered: std::collections::BTreeSet<&str> = page
        .split("<input")
        .skip(1)
        .map(|tag| &tag[..tag.find('>').unwrap_or(tag.len())])
        .filter(|tag| tag.contains("name=\"difficulty\""))
        .filter_map(|tag| tag.split("value=\"").nth(1))
        .filter_map(|value| value.split('"').next())
        .collect();
    assert!(
        !offered.is_empty(),
        "web/index.html offers no difficulty filter at all"
    );

    for chunk in page.split("data-difficulty=\"").skip(1) {
        let level = chunk.split('"').next().expect("data-difficulty is quoted");
        assert!(
            offered.contains(level),
            "web/index.html has a {level} problem card, which the difficulty filter never shows"
        );
    }
}

/// A recording is delivered to a person, and a typed handle is not one.
///
/// The refusal lives on `/api/token` rather than on the delivery step because
/// the alternative is a candidate who completes an interview that can never be
/// sent anywhere, which is the worst moment to find out.
#[tokio::test]
async fn token_requires_verified_recording_identity() {
    let (mut config, cookie, path) = signed_in_web_config("recording-identity");
    config.recording = Some(recording_config());
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();

    let refused = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    let body = refused.json::<Value>().await.unwrap();
    assert_eq!(body["code"], "recording_requires_verified_identity");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()),
        "the candidate needs a sentence, not only a code"
    );

    // The same account, once the row carries a verified address, starts an
    // interview. Written directly because this test is about the gate, not
    // about how the column is filled; the callback test covers that path.
    // Without this half the test would pass against a server that refused
    // everyone.
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE users SET email = 'one@example.test', email_verified = 1 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    let interview = start_interview(&client, &base, &cookie).await;
    let allowed = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie)
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);

    server.abort();
    remove_database(path);
}

/// An address nobody checked is not a delivery target, and a typed handle can
/// never acquire one.
#[tokio::test]
async fn self_declared_handle_cannot_authorize_delivery() {
    let db_path = account_db_path("self-declared-delivery");
    initialize_account_database(&db_path).unwrap();
    let mut config = web_config();
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(db_path.clone());
    config.recording = Some(recording_config());
    let (base, server) = spawn_web_server(config).await;
    let client = reqwest::Client::new();

    let cookie = record_login(&client, &base, "octocat").await;
    let refused = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert_eq!(
        refused.json::<Value>().await.unwrap()["code"],
        "recording_requires_verified_identity"
    );

    // Typing the same handle again mints another account rather than adopting
    // the first, so nobody can accumulate their way into a verified one. The
    // row count below is the assertion; `record_login` already fails if the
    // login itself did not succeed.
    record_login(&client, &base, "octocat").await;

    let connection = rusqlite::Connection::open(&db_path).unwrap();
    let mut statement = connection
        .prepare("SELECT github_id, email, email_verified FROM users ORDER BY github_id")
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 2, "two logins, two accounts, never an upgrade");
    for (github_id, email, verified) in rows {
        assert!(github_id < 0, "a self-declared account keeps a negative id");
        assert_eq!(email, None, "and no address");
        assert_eq!(verified, 0, "and nothing claiming one was checked");
    }

    drop(statement);
    drop(connection);
    server.abort();
    remove_database(db_path);
}

/// An OAuth sign-in stores the primary verified address, and only that one.
#[tokio::test]
async fn github_callback_stores_only_the_primary_verified_email() {
    let (github_base, github_server) = spawn_mock_github().await;
    let path = account_db_path("verified-email");
    initialize_account_database(&path).unwrap();
    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    config.github_oauth_base_url = Some(github_base.clone());
    config.github_api_base_url = Some(github_base);
    config.recording = Some(recording_config());
    let (base, server) = spawn_web_server(config).await;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let response = client
        .get(format!("{base}/api/callback?code=ok&state=state-token"))
        .header(
            "cookie",
            format!(
                "codetrial_oauth_state={}",
                signed_cookie("state-token", "session-secret")
            ),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 302);
    let session_cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|cookie| cookie.starts_with("codetrial_session="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let (email, verified): (Option<String>, i64) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT email, email_verified FROM users WHERE github_id = 303",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        email.as_deref(),
        Some("octocat@example.test"),
        "the primary verified address, not the first entry and not the secondary one"
    );
    assert_eq!(verified, 1);

    // And that account can now start a recorded interview.
    let client = reqwest::Client::new();
    let interview = start_interview(&client, &base, &session_cookie).await;
    let allowed = client
        .post(format!("{base}/api/token"))
        .header("cookie", session_cookie)
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);

    server.abort();
    github_server.abort();
    remove_database(path);
}

/// The scope has to name what the callback then asks for. A widened fetch
/// against an unwidened authorize URL fails at GitHub, not here, and the
/// symptom is an address that is simply absent.
#[tokio::test]
async fn login_requests_the_email_scope_it_later_reads() {
    let db_path = account_db_path("email-scope");
    initialize_account_database(&db_path).unwrap();
    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(db_path.clone());
    let without_recording = config.clone();
    config.recording = Some(recording_config());
    let (base, server) = spawn_web_server(config).await;

    let authorize_url = |base: String| async move {
        let response = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
            .get(format!("{base}/api/login"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 302);
        response
            .headers()
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    };
    let location = authorize_url(base).await;

    // Decoded, not matched as an escaped literal. Percent-encoding has more
    // than one legal answer for a colon, and the thing that matters is the
    // scope GitHub will read.
    assert_eq!(
        query_parameter(&location, "scope").as_deref(),
        Some("read:user user:email"),
        "the authorize URL has to request the scope the callback then reads: {location}"
    );
    server.abort();

    // And a deployment that records nothing does not put a candidate's private
    // address on the consent screen, because it would never read it.
    let (base, server) = spawn_web_server(without_recording).await;
    assert_eq!(
        query_parameter(&authorize_url(base).await, "scope").as_deref(),
        Some("read:user")
    );

    server.abort();
    remove_database(db_path);
}

/// Agrees to the recording notice the way the browser does, and hands back the
/// interview id `/api/token` then has to be given.
async fn start_interview(client: &reqwest::Client, base: &str, cookie: &str) -> String {
    let response = client
        .post(format!("{base}/api/interviews"))
        .header("cookie", cookie)
        .json(&json!({ "consentVersion": codetrial::recording::CONSENT_VERSION }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201, "consent should be recorded");
    response.json::<Value>().await.unwrap()["interviewId"]
        .as_str()
        .expect("the response carries the interview id")
        .to_string()
}

/// A recording block for tests that only care that recording is on.
/// The lobby offers lengths this deployment may not be able to record, and only
/// the server knows the cap. Without it in the payload the browser goes on
/// offering sixty minutes and `/api/token` shortens the interview after the
/// candidate has already asked for it, which is the substitution #16 is about.
#[tokio::test]
async fn the_session_reports_how_long_a_recorded_interview_may_run() {
    let mut config = web_config();
    config.recording = Some(recording_config());
    let (base, server) = spawn_web_server(config).await;

    let session = reqwest::get(format!("{base}/api/session"))
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();

    assert_eq!(session["maxDurationMin"], recording_config().max_minutes);
    server.abort();
}

/// The same endpoint on a server that records nothing, so the cap is the range
/// the endpoint has always offered rather than a number the lobby invents.
#[tokio::test]
async fn a_server_that_does_not_record_caps_nothing_beyond_the_range() {
    let (base, server) = spawn_web_server(web_config()).await;

    let session = reqwest::get(format!("{base}/api/session"))
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();

    assert_eq!(session["maxDurationMin"], MAX_DURATION_MIN);
    server.abort();
}

fn recording_config() -> codetrial::config::RecordingConfig {
    codetrial::config::RecordingConfig {
        livekit: None,
        gcs_bucket: "codetrial-staging".to_string(),
        gcs_prefix: "codetrial".to_string(),
        drive_id: "0AKfixtureDriveId".to_string(),
        service_account_json: "{}".to_string(),
        max_minutes: 45,
        bitrate: 2000,
        kill_switch: false,
        template_base_url: "https://recording.codetrial.example".to_string(),
    }
}

/// A verified address is not given up because somebody else's server had a bad
/// minute.
///
/// The callback upserts whatever it was handed, so folding a rate limit into
/// "this account has no verified address" would take a candidate's delivery
/// target away permanently, on a transient failure they cannot see or retry.
#[tokio::test]
async fn a_failed_email_lookup_does_not_unverify_an_account() {
    let path = account_db_path("email-lookup-failure");
    initialize_account_database(&path).unwrap();

    // One good sign-in first, so there is something to lose.
    let (good_github, good_server) = spawn_mock_github().await;
    let mut config = web_config();
    config.github_client_id = Some("client".to_string());
    config.github_client_secret = Some("secret".to_string());
    config.session_secret = Some("session-secret".to_string());
    config.db_path = Some(path.clone());
    config.github_oauth_base_url = Some(good_github.clone());
    config.github_api_base_url = Some(good_github);
    config.recording = Some(recording_config());
    let (base, server) = spawn_web_server(config.clone()).await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let callback = |base: String| {
        let client = client.clone();
        async move {
            client
                .get(format!("{base}/api/callback?code=ok&state=state-token"))
                .header(
                    "cookie",
                    format!(
                        "codetrial_oauth_state={}",
                        signed_cookie("state-token", "session-secret")
                    ),
                )
                .send()
                .await
                .unwrap()
        }
    };
    assert_eq!(callback(base.clone()).await.status(), 302);
    server.abort();
    good_server.abort();

    // Now the same account signs in again while /user/emails is rate limited.
    let (rate_limited, rate_limited_server) = spawn_rate_limited_github().await;
    let mut second = config;
    second.github_oauth_base_url = Some(rate_limited.clone());
    second.github_api_base_url = Some(rate_limited);
    let (base, server) = spawn_web_server(second).await;
    assert_eq!(
        callback(base).await.status(),
        502,
        "a failed lookup fails the sign-in rather than downgrading the account"
    );

    let (email, verified): (Option<String>, i64) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT email, email_verified FROM users WHERE github_id = 303",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(email.as_deref(), Some("octocat@example.test"));
    assert_eq!(verified, 1, "the address survives the outage");

    server.abort();
    rate_limited_server.abort();
    remove_database(path);
}

/// GitHub with a working `/user` and a rate-limited `/user/emails`. The 403
/// body is JSON and parses, which is what makes this failure look like an
/// answer rather than an error to anything that does not check the status.
async fn spawn_rate_limited_github() -> (String, tokio::task::JoinHandle<Result<(), std::io::Error>>)
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = axum::Router::new()
        .route(
            "/login/oauth/access_token",
            axum::routing::post(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    json!({"access_token":"mock-token"}).to_string(),
                )
            }),
        )
        .route(
            "/user",
            axum::routing::get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    json!({"id":303,"login":"octocat","avatar_url":"https://example.test/avatar.png"}).to_string(),
                )
            }),
        )
        .route(
            "/user/emails",
            axum::routing::get(|| async {
                (
                    axum::http::StatusCode::FORBIDDEN,
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    json!({"message":"API rate limit exceeded"}).to_string(),
                )
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    (format!("http://{addr}"), server)
}

/// The `scope` parameter of a redirect location, percent-decoded.
fn query_parameter(location: &str, name: &str) -> Option<String> {
    let query = location.split_once('?')?.1;
    let value = query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{name}=")))?;
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
                decoded.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

/// Consent exists before anything could be recorded, and `/api/token` is where
/// that is enforced.
///
/// The token is what precedes an Egress call, so a token minted without a
/// persisted consent row is the one ordering this feature cannot survive. The
/// enforcement is the refusal; a comment saying "call this first" is not.
#[tokio::test]
async fn interviews_persist_consent_before_egress() {
    let (base, server, path, client, cookie) = recorded_server("consent-before-egress").await;

    // No interview id: refused, and nothing is written.
    let refused = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert_eq!(
        refused.json::<Value>().await.unwrap()["code"],
        "recording_requires_consent"
    );
    assert_eq!(interview_rows(&path).len(), 0);

    // A stale page that shows older wording is refused rather than recorded as
    // having agreed to text it never displayed.
    let stale = client
        .post(format!("{base}/api/interviews"))
        .header("cookie", cookie.clone())
        .json(&json!({ "consentVersion": "1970-01-01" }))
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), 409);
    assert_eq!(
        stale.json::<Value>().await.unwrap()["code"],
        "consent_version_mismatch"
    );
    assert_eq!(interview_rows(&path).len(), 0);

    let interview = start_interview(&client, &base, &cookie).await;
    let rows = interview_rows(&path);
    assert_eq!(
        rows.len(),
        1,
        "consent is one row, written before the token"
    );
    let (id, version, consent_at, withdrawn) = rows[0].clone();
    assert_eq!(id, interview);
    assert_eq!(version, codetrial::recording::CONSENT_VERSION);
    assert!(consent_at > 0, "consent is a fact with a time");
    assert_eq!(withdrawn, None);

    // A reload is not a second consent. Same person, same wording, same
    // interview that has not started, so the pending row is handed back rather
    // than spending another of the account's rows for nothing.
    assert_eq!(
        start_interview(&client, &base, &cookie).await,
        interview,
        "agreeing again before starting returns the pending interview"
    );
    assert_eq!(interview_rows(&path).len(), 1);

    let allowed = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(allowed.status(), 200);

    // One consent is one interview. Without the claim, the same id mints
    // recorded rooms without limit, and a candidate who agreed to be recorded
    // once would have agreed to all of them.
    let reused = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(reused.status(), 403);
    assert_eq!(
        reused.json::<Value>().await.unwrap()["code"],
        "recording_requires_consent"
    );

    // The room the consent authorized is written down, so a later step can find
    // the consent a room was started under.
    let room: Option<String> = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT room_name FROM interviews WHERE id = ?1",
            [&interview],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        room.is_some_and(|room| room.starts_with("interview-")),
        "the claim records which room it was spent on"
    );

    // Somebody else's interview id is not consent. It answers the same way a
    // missing one does, because telling them apart enumerates other people's
    // interviews.
    let (other_cookie, _) = second_account(&client, &base, &path).await;
    let other = start_interview(&client, &base, &other_cookie).await;
    let borrowed = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie)
        .json(&json!({ "interviewId": other }))
        .send()
        .await
        .unwrap();
    assert_eq!(borrowed.status(), 403);

    server.abort();
    remove_database(path);
}

/// Withdrawal is recorded, and a withdrawn interview cannot start another
/// recorded room.
///
/// The transition of an active recording to `failed` belongs to the recording
/// lifecycle. What this route owns is the state that transition reads, and the
/// refusal that stops the candidate who just said no from reloading into a
/// second recorded interview.
#[tokio::test]
async fn consent_withdrawal_stops_egress() {
    let provider = std::sync::Arc::new(FakeRecordingProvider::default());
    let (base, server, path, client, cookie) =
        recorded_server_with_provider("consent-withdrawal", provider.clone()).await;
    let interview = start_interview(&client, &base, &cookie).await;

    // A real recording, started through the route a candidate reaches, so the
    // withdrawal below has something to stop rather than an empty interview.
    let token = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(token.status(), 200);
    let started = client
        .post(format!("{base}/api/interviews/{interview}/recording"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(started.status(), 202, "the provider has been asked");
    assert_eq!(
        started.json::<Value>().await.unwrap()["state"],
        "recording",
        "the row moved when the egress id landed, and the answer says so"
    );
    assert_eq!(provider.starts(), 1);

    let withdrawn = client
        .delete(format!("{base}/api/interviews/{interview}/consent"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(withdrawn.status(), 204);

    // The recording is abandoned, not finished: `failed` with the reason a
    // later deletion is justified by, and the provider told to stop.
    assert_eq!(provider.stops().len(), 1, "the Egress job is stopped");
    let (state, error): (String, Option<String>) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT state, error FROM recordings WHERE interview_id = ?1",
            [&interview],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "failed");
    assert_eq!(error.as_deref(), Some("consent_withdrawn"));

    let rows = interview_rows(&path);
    assert_eq!(rows.len(), 1, "withdrawal records, it does not delete");
    let first_withdrawal = rows[0].3.expect("the withdrawal has a time");

    // Idempotent, and the first time is the one that counts.
    let again = client
        .delete(format!("{base}/api/interviews/{interview}/consent"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 204);
    assert_eq!(interview_rows(&path)[0].3, Some(first_withdrawal));

    let refused = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 403);
    assert_eq!(
        refused.json::<Value>().await.unwrap()["code"],
        "recording_requires_consent"
    );

    // Nobody else can withdraw it, and the refusal does not confirm it exists.
    let (other_cookie, _) = second_account(&client, &base, &path).await;
    let mine = start_interview(&client, &base, &cookie).await;
    let stranger = client
        .delete(format!("{base}/api/interviews/{mine}/consent"))
        .header("cookie", other_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(stranger.status(), 404);
    assert_eq!(
        interview_rows(&path)
            .iter()
            .find(|(id, ..)| *id == mine)
            .expect("the interview is still there")
            .3,
        None,
        "a stranger's request changes nothing"
    );

    server.abort();
    remove_database(path);
}

/// The router's whole route list, against a fixed allowlist.
///
/// Equality, not containment. The property worth defending is that no route
/// takes media: LiveKit writes the file straight to the staging bucket and
/// CodeTrial never touches the bytes, and "there is no upload route" is a
/// negative that only an exhaustive list can observe. A containment check would
/// pass while an ingest route sat beside the ones it named.
///
/// Read as text out of `src/web/mod.rs` because axum does not hand back the
/// routes it was given. That makes this a tripwire on the source rather than
/// on the running router, which is the trade the whole file already makes for
/// the browser side.
///
/// The ceiling, stated because the name overpromises otherwise: this compares
/// paths. It does not read methods, and it cannot see what a handler does with
/// a body. What it establishes is that the set of paths is the set somebody
/// wrote down, which is what makes "there is no upload route" checkable at
/// all.
/// Every path `web_router` declares, sorted.
///
/// Shared rather than scanned twice: the owner-scope test below needs the same
/// list, and a second scan is a second chance to disagree about what the
/// router holds. The greedy version it started with took the first quote after
/// each `.route(`, which silently aliased both constant-named routes onto the
/// next literal in the file.
fn declared_routes() -> Vec<String> {
    let source = fs::read_to_string("src/web/mod.rs").unwrap();
    let router = source
        .split_once("    Router::new()")
        .expect("web_router builds a Router")
        .1
        .split_once("        .fallback(")
        .expect("the static handler is the fallback")
        .0;

    // This is only exhaustive while every route arrives through `.route(`. A
    // nested or merged router would add paths this scan cannot see, so adding
    // one has to fail here rather than pass quietly.
    for composed in [".nest(", ".nest_service(", ".merge(", ".route_service("] {
        assert!(
            !router.contains(composed),
            "{composed} adds routes this test cannot enumerate; extend it before using one"
        );
    }

    // The first argument of every `.route(` call. Two of them are constants
    // rather than literals, because the contract fixes those paths and
    // `src/recording.rs` owns them, so they are resolved rather than matched.
    let mut routes = router
        .split(".route(")
        .skip(1)
        .map(|rest| {
            let rest = rest.trim_start();
            if rest.starts_with("crate::recording::WEBHOOK_ROUTE") {
                return codetrial::recording::WEBHOOK_ROUTE.to_string();
            }
            if rest.starts_with("crate::recording::REPLAY_ROUTE") {
                return codetrial::recording::REPLAY_ROUTE.to_string();
            }
            rest.trim_start_matches('"')
                .split('"')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect::<Vec<_>>();
    routes.sort();
    routes
}

#[test]
fn router_routes_match_a_fixed_allowlist() {
    let routes = declared_routes();
    let mut allowed = vec![
        "/healthz",
        "/runtime-config.js",
        "/api/token",
        "/api/observer-token",
        "/api/login",
        "/api/callback",
        "/api/session",
        "/api/logout",
        "/api/recordings",
        "/api/recordings/{id}",
        "/api/recordings/{id}/events",
        "/api/reports",
        "/api/interviews",
        "/api/interviews/{id}/consent",
        "/api/interviews/{id}/recording",
        "/api/interviews/{id}/end",
        "/api/interviews/{id}/events",
        "/api/interviews/{id}/snapshot",
        "/api/recording/replay",
        "/api/recording/webhook",
    ];
    allowed.sort_unstable();

    assert_eq!(
        routes, allowed,
        "the route list changed; a new one has to be added here deliberately, and none of them \
         may accept media"
    );
}

/// A second start does not make a second Egress job, and is not told the
/// recording has not begun.
///
/// The row moves underneath the handler: `starting` becomes `recording` when
/// the first caller's egress id lands. A second caller that answered from the
/// snapshot it read would tell the browser to keep waiting for a recording that
/// was already running.
#[tokio::test]
async fn a_second_start_reports_the_state_the_row_holds() {
    let provider = std::sync::Arc::new(FakeRecordingProvider::default());
    let (base, server, path, client, cookie) =
        recorded_server_with_provider("second-start", provider.clone()).await;
    let interview = start_interview(&client, &base, &cookie).await;
    assert_eq!(
        client
            .post(format!("{base}/api/token"))
            .header("cookie", cookie.clone())
            .json(&json!({ "interviewId": interview }))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    let start = || {
        let client = client.clone();
        let base = base.clone();
        let cookie = cookie.clone();
        let interview = interview.clone();
        async move {
            client
                .post(format!("{base}/api/interviews/{interview}/recording"))
                .header("cookie", cookie)
                .send()
                .await
                .unwrap()
        }
    };

    let first = start().await;
    assert_eq!(first.status(), 202);
    assert_eq!(first.json::<Value>().await.unwrap()["state"], "recording");

    let second = start().await;
    assert_eq!(second.status(), 202);
    assert_eq!(
        second.json::<Value>().await.unwrap()["state"],
        "recording",
        "the second caller reads the row, not the snapshot it started from"
    );
    assert_eq!(provider.starts(), 1, "one interview, one Egress job");

    server.abort();
    remove_database(path);
}

/// The row moves while the provider is answering, and the answer says so.
///
/// This is the interleaving the snapshot hid: the handler reads a `starting`
/// row, a sweeper retry stores its own egress id, and the handler comes back
/// with an id the row will not take. It has to stop the job it made and report
/// the state that is actually there.
#[tokio::test]
async fn a_start_overtaken_mid_flight_stops_its_own_job() {
    let provider = std::sync::Arc::new(FakeRecordingProvider::default());
    let (base, server, path, client, cookie) =
        recorded_server_with_provider("overtaken", provider.clone()).await;
    let interview = start_interview(&client, &base, &cookie).await;
    assert_eq!(
        client
            .post(format!("{base}/api/token"))
            .header("cookie", cookie.clone())
            .json(&json!({ "interviewId": interview }))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    *provider.preempt.lock().unwrap() = Some(path.clone());

    let response = client
        .post(format!("{base}/api/interviews/{interview}/recording"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    assert_eq!(
        response.json::<Value>().await.unwrap()["state"],
        "recording",
        "the state comes from the row, which somebody else moved"
    );

    // The job this call made is not the job the row names, so it stops its own
    // rather than leaving two running.
    assert_eq!(provider.stops(), vec!["EG_web_fake".to_string()]);
    let egress: Option<String> = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT egress_id FROM recordings WHERE interview_id = ?1",
            [&interview],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        egress.as_deref(),
        Some("EG_swept"),
        "the id that got there first is the one that stays"
    );

    server.abort();
    remove_database(path);
}

/// A completion carrying nothing is a failure, driven through the real webhook.
///
/// The predicate is unit tested; this is the path a candidate's recording
/// actually takes, signature and all. Sending an empty completion through the
/// transfer shared a zero-byte video.
#[tokio::test]
async fn a_completion_with_no_file_fails_the_recording() {
    let provider = std::sync::Arc::new(FakeRecordingProvider::default());
    let (base, server, path, client, cookie) =
        recorded_server_with_provider("empty-completion", provider.clone()).await;
    let interview = start_interview(&client, &base, &cookie).await;
    assert_eq!(
        client
            .post(format!("{base}/api/token"))
            .header("cookie", cookie.clone())
            .json(&json!({ "interviewId": interview }))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .post(format!("{base}/api/interviews/{interview}/recording"))
            .header("cookie", cookie.clone())
            .send()
            .await
            .unwrap()
            .status(),
        202
    );

    let ended = json!({
        "event": "egress_ended",
        "id": "EV_empty",
        "createdAt": "1770000123",
        "egressInfo": {
            "egressId": "EG_web_fake",
            "status": "EGRESS_COMPLETE",
            "fileResults": []
        }
    })
    .to_string();
    let response = post_webhook(&client, &base, &ended).await;
    assert_eq!(response.status(), 200);

    let (state, error): (String, Option<String>) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT state, error FROM recordings WHERE interview_id = ?1",
            [&interview],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, "failed");
    assert_eq!(error.as_deref(), Some("partial_output"));

    // And the status route says what to do about it, which "failed" does not.
    let status = client
        .get(format!("{base}/api/interviews/{interview}/recording"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), 200);
    let body = status.json::<Value>().await.unwrap();
    assert_eq!(body["error"], "partial_output");
    assert_eq!(body["recovery"], "start_again");
    for handle in ["gcsObject", "driveFileId", "drivePermissionId"] {
        assert!(
            body.get(handle).is_none(),
            "the status route must not hand back {handle}"
        );
    }

    server.abort();
    remove_database(path);
}

/// Signs a webhook body the way LiveKit does and posts it.
///
/// The signature covers the exact bytes, so the body is passed as a string and
/// never re-serialized between here and the request.
async fn post_webhook(client: &reqwest::Client, base: &str, body: &str) -> reqwest::Response {
    use base64::Engine as _;
    use sha2::Digest as _;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = json!({
        "iss": "devkey",
        "nbf": now,
        "exp": now + 300,
        "sha256": base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(body.as_bytes())),
    });
    let header = URL_SAFE_NO_PAD.encode(json!({ "alg": "HS256", "typ": "JWT" }).to_string());
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    let signing_input = format!("{header}.{payload}");
    let mut mac = HmacSha256::new_from_slice(b"devsecret").unwrap();
    mac.update(signing_input.as_bytes());
    let token = format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    );

    client
        .post(format!("{base}{}", codetrial::recording::WEBHOOK_ROUTE))
        .header("authorization", token)
        .header("content-type", "application/webhook+json")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
}

/// A malformed body is a client error, whether or not this server records.
///
/// The consent check reads the body too, and it answers 403 to anything it
/// cannot parse. Letting that answer win sends a candidate looking for a
/// checkbox they already ticked.
#[tokio::test]
async fn a_malformed_body_is_a_bad_request_even_on_a_recording_server() {
    let (base, server, path, client, cookie) = recorded_server("malformed-recorded").await;
    start_interview(&client, &base, &cookie).await;

    let response = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie)
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"],
        "Session request must be JSON."
    );

    server.abort();
    remove_database(path);
}

/// A request that fails after the consent check must not spend the consent.
///
/// The claim used to run before the token was signed and before an interviewer
/// was dispatched, so a busy server burned the candidate's only consent and
/// then told them to retry, and the retry was refused.
#[tokio::test]
async fn a_refused_interview_leaves_its_consent_unspent() {
    let (mut config, cookie, path) = signed_in_web_config("consent-unspent");
    config.recording = Some(recording_config());
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE users SET email = 'one@example.test', email_verified = 1 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    let dispatcher = std::sync::Arc::new(RecordingDispatcher::default());
    dispatcher
        .at_capacity
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let (base, server) = spawn_web_server_with_dispatcher(config, dispatcher.clone()).await;
    let client = reqwest::Client::new();

    let interview = start_interview(&client, &base, &cookie).await;
    let busy = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie.clone())
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(busy.status(), 503, "the server is full, not the candidate");

    let room: Option<String> = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT room_name FROM interviews WHERE id = ?1",
            [&interview],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(room, None, "a refused request spends nothing");

    // And the retry the candidate was told to make works.
    dispatcher
        .at_capacity
        .store(false, std::sync::atomic::Ordering::Relaxed);
    let retried = client
        .post(format!("{base}/api/token"))
        .header("cookie", cookie)
        .json(&json!({ "interviewId": interview }))
        .send()
        .await
        .unwrap();
    assert_eq!(retried.status(), 200);

    server.abort();
    remove_database(path);
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

    let template = reqwest::get(format!("{base}{}", codetrial::recording::TEMPLATE_PATH))
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

    server.abort();
}

/// The per-batch caps bound one request and the stored quota bounds what an
/// interview keeps. Neither bounds how often one account may ask, and an ask
/// that is not refused before the body is a parse and a stored batch. This is
/// the refusal.
#[tokio::test]
async fn replay_ingestion_rate_limits_a_looping_client() {
    let (base, server, path, client, cookie) = recorded_server("replay-rate-limit").await;
    let interview = start_interview(&client, &base, &cookie).await;
    let url = format!("{base}/api/interviews/{interview}/events");
    let batch = json!({
        "events": [{
            "v": codetrial::recording::REPLAY_VERSION,
            "kind": "transcript",
            "at": 1_770_000_000_000i64,
            "payload": { "text": "hello" }
        }]
    });

    for attempt in 1..=REPLAY_RATE_LIMIT {
        let response = client
            .post(&url)
            .header("cookie", &cookie)
            .json(&batch)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "batch {attempt} should be allowed");
    }

    let blocked = client
        .post(&url)
        .header("cookie", &cookie)
        .json(&batch)
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 429);
    assert_eq!(blocked.headers().get("retry-after").unwrap(), "60");

    // Refused before the body, so the refused batch stored nothing. A limiter
    // that ran after the insert would answer 429 to a client whose events it
    // had already kept.
    let stored: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM replay_events WHERE interview_id = ?1",
            [&interview],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, i64::from(REPLAY_RATE_LIMIT));

    server.abort();
    remove_database(path);
}

/// The replay is the account's own, and only the account's.
///
/// The route is the one place an interview id arrives from a browser, so the
/// owner check is what stands between a replay and anyone who can guess an id.
#[tokio::test]
async fn replay_events_are_owner_scoped() {
    let (base, server, path, client, cookie) = recorded_server("replay-owner").await;
    let interview = start_interview(&client, &base, &cookie).await;

    let event = json!({
        "v": codetrial::recording::REPLAY_VERSION,
        "kind": "transcript",
        "at": 1_770_000_000_000i64,
        "payload": { "text": "hello" }
    });
    let stored = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [event, event] }))
        .send()
        .await
        .unwrap();
    assert_eq!(stored.status(), 200);
    let body = stored.json::<Value>().await.unwrap();
    assert_eq!(
        (body["firstSeq"].as_i64(), body["lastSeq"].as_i64()),
        (Some(0), Some(1))
    );

    // Somebody else's interview, by id. Not a 403: an account that does not own
    // an interview should not learn that it exists.
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "INSERT INTO users (id, github_id, login, email, email_verified, created_at, updated_at)
                   VALUES (2, 202, 'two', 'two@example.test', 1, 1, 1);
                 INSERT INTO interviews (id, account_id, consent_version, consent_at)
                   VALUES ('int-theirs', 2, '2026-08-21', 1);",
            )
            .unwrap();
    }
    let theirs = client
        .post(format!("{base}/api/interviews/int-theirs/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [event] }))
        .send()
        .await
        .unwrap();
    assert_eq!(theirs.status(), 404);
    let count: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM replay_events WHERE interview_id = 'int-theirs'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);

    // And a batch nobody sent by hand.
    let flood: Vec<Value> = std::iter::repeat_n(event.clone(), 200).collect();
    let refused = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": flood }))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 413);

    // A batch with nothing in it gets no range of sequence numbers, because
    // none were allocated.
    let empty = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(empty.status(), 400);
    assert_eq!(
        empty.json::<Value>().await.unwrap()["code"],
        "replay_envelope_invalid"
    );

    let signed_out = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .json(&json!({ "events": [event] }))
        .send()
        .await
        .unwrap();
    assert_eq!(signed_out.status(), 401);

    // The byte ceiling, over HTTP, with one row standing in for eight megabytes
    // of replay: what is under test is the refusal and the flag, not SQLite's
    // ability to hold five thousand rows.
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        ) VALUES ('rec-replay', 1, ?1, 'interview-abc12345', ?1,
            'one@example.test', 'recording', 1, 1)
        ",
                [&interview],
            )
            .unwrap();
        connection
            .execute(
                "
        INSERT INTO replay_events (interview_id, seq, kind, at, payload, bytes, received_at)
        VALUES (?1, 9999, 'transcript', 1, '{}', ?2, 1)
        ",
                rusqlite::params![&interview, codetrial::recording::MAX_REPLAY_BYTES],
            )
            .unwrap();
    }
    let over = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [event] }))
        .send()
        .await
        .unwrap();
    assert_eq!(over.status(), 413);
    assert_eq!(
        over.json::<Value>().await.unwrap()["code"],
        "replay_quota_exceeded"
    );
    let (state, flag): (String, i64) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT state, quota_exceeded FROM recordings WHERE id = 'rec-replay'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        (state.as_str(), flag),
        ("recording", 1),
        "the replay stopped growing; the recording did not fail"
    );

    // Withdrawal closes ingest. A browser with a buffer will flush it after the
    // candidate has said stop, and a replay that keeps growing past a
    // withdrawal is not a withdrawal.
    {
        let withdrawn = client
            .delete(format!("{base}/api/interviews/{interview}/consent"))
            .header("cookie", cookie.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(withdrawn.status(), 204);
    }
    let flushed = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [event] }))
        .send()
        .await
        .unwrap();
    assert_eq!(flushed.status(), 404);
    let stored_after: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM replay_events WHERE interview_id = ?1",
            [&interview],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        stored_after, 3,
        "the two stored above plus the synthetic quota row, and nothing after the withdrawal"
    );

    server.abort();
    remove_database(path);
}

/// The recorder reads the replay with the credential it was given.
///
/// The Egress browser holds no session cookie, so this is the one replay read
/// authorized by a room token. What matters is that the token is verified
/// rather than parsed, and that it only opens the room it names.
#[tokio::test]
async fn the_recorder_reads_the_replay_for_the_room_its_token_names() {
    let (base, server, path, client, cookie) = recorded_server("replay-room-token").await;
    let interview = start_interview(&client, &base, &cookie).await;
    let room = "interview-abc12345";
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        ) VALUES ('rec-room', 1, ?1, ?2, ?1, 'one@example.test', 'recording', 1, 1)
        ",
            [&interview, &room.to_string()],
        )
        .unwrap();
    let event = |kind: &str, text: &str| {
        json!({
            "v": codetrial::recording::REPLAY_VERSION,
            "kind": kind,
            "at": 1_770_000_000_000i64,
            "payload": { "text": text }
        })
    };
    let posted = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [
            event("editor", "first draft"),
            event("transcript", "hello"),
            event("editor", "second draft"),
        ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), 200);

    let token_for = |room: &str| {
        codetrial::token::livekit_token(codetrial::token::LivekitTokenInput {
            api_key: "devkey",
            api_secret: "devsecret",
            identity: "EG_recorder",
            name: "recorder",
            room,
            metadata: "",
            agent: false,
            now_seconds: codetrial::current_epoch_seconds(),
        })
        .unwrap()
    };
    let replay = |token: String, query: &str| {
        let url = format!("{base}/api/recording/replay{query}");
        let client = client.clone();
        async move {
            client
                .get(url)
                .header("authorization", token)
                .send()
                .await
                .unwrap()
        }
    };

    let snapshot = replay(token_for(room), "").await;
    assert_eq!(snapshot.status(), 200);
    let body = snapshot.json::<Value>().await.unwrap();
    assert_eq!(body["seq"], 2);
    assert_eq!(
        body["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["transcript", "editor"],
        "the snapshot drops the superseded editor frame"
    );

    // The tail keeps every frame, superseded or not: a reader carrying on from
    // a snapshot is replaying, and a frame it never saw is not one to skip.
    let tail = replay(token_for(room), "?after=0").await;
    assert_eq!(tail.status(), 200);
    assert_eq!(
        tail.json::<Value>().await.unwrap()["events"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // A token for another room opens nothing here.
    let elsewhere = replay(token_for("interview-def67890"), "").await;
    assert_eq!(elsewhere.status(), 404);

    // Forged and absent credentials are the same answer.
    let forged = codetrial::token::livekit_token(codetrial::token::LivekitTokenInput {
        api_key: "devkey",
        api_secret: "not-the-secret",
        identity: "EG_recorder",
        name: "recorder",
        room,
        metadata: "",
        agent: false,
        now_seconds: codetrial::current_epoch_seconds(),
    })
    .unwrap();
    assert_eq!(replay(forged, "").await.status(), 401);
    assert_eq!(
        client
            .get(format!("{base}/api/recording/replay"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );

    server.abort();
    remove_database(path);
}

/// The history a candidate reads about their own interviews.
#[tokio::test]
async fn history_lists_own_recordings_only() {
    let (base, server, path, client, cookie) = recorded_server("history-lists").await;
    let interview = start_interview(&client, &base, &cookie).await;
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "INSERT INTO users (id, github_id, login, email, email_verified, created_at, updated_at)
                   VALUES (2, 202, 'two', 'two@example.test', 1, 1, 1);
                 INSERT INTO interviews (id, account_id, consent_version, consent_at)
                   VALUES ('int-theirs', 2, '2026-08-21', 1);",
            )
            .unwrap();
        connection
            .execute(
                "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at, ready_at, expires_at
        ) VALUES ('rec-mine', 1, ?1, 'interview-abc12345', ?1,
            'one@example.test', 'ready', 10, 10, 12, 99999999999)
        ",
                [&interview],
            )
            .unwrap();
        connection
            .execute(
                "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        ) VALUES ('rec-theirs', 2, 'int-theirs', 'interview-def67890', 'int-theirs',
            'two@example.test', 'ready', 11, 11)
        ",
                [],
            )
            .unwrap();
    }

    let listed = client
        .get(format!("{base}/api/recordings"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(listed.status(), 200);
    let body = listed.json::<Value>().await.unwrap();
    assert_eq!(
        body["recordings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["recordingId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["rec-mine"],
        "somebody else's interview is not in this account's history"
    );
    assert_eq!(body["nextCursor"], Value::Null, "one page holds one row");

    // The detail carries no handle to media: not the object path, not the Drive
    // file, not the permission.
    let detail = client
        .get(format!("{base}/api/recordings/rec-mine"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(detail.status(), 200);
    let detail = detail.json::<Value>().await.unwrap();
    assert_eq!(detail["state"], "ready");
    assert_eq!(detail["readyAt"], 12);
    let printed = detail.to_string();
    for handle in ["gcsObject", "driveFileId", "drivePermissionId", "roomName"] {
        assert!(
            !printed.contains(handle),
            "{handle} is a handle to media: {printed}"
        );
    }

    server.abort();
    remove_database(path);
}

#[tokio::test]
async fn history_cross_account_denied() {
    let (base, server, path, client, cookie) = recorded_server("history-cross").await;
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "INSERT INTO users (id, github_id, login, email, email_verified, created_at, updated_at)
                   VALUES (2, 202, 'two', 'two@example.test', 1, 1, 1);
                 INSERT INTO interviews (id, account_id, consent_version, consent_at)
                   VALUES ('int-theirs', 2, '2026-08-21', 1);
                 INSERT INTO recordings (
                     id, account_id, interview_id, room_name, idempotency_key,
                     recipient_email, state, created_at, updated_at
                 ) VALUES ('rec-theirs', 2, 'int-theirs', 'interview-def67890', 'int-theirs',
                     'two@example.test', 'ready', 11, 11);
                 INSERT INTO replay_events (interview_id, seq, kind, at, payload, bytes, received_at)
                   VALUES ('int-theirs', 0, 'transcript', 1, '{}', 2, 1);",
            )
            .unwrap();
    }

    // Not `403`: an account that does not own a recording should not learn that
    // it exists. The two paths are refused by two different scopings, which is
    // the point of asking both: the detail is scoped by `recording_summary`'s
    // `account_id`, and the events by the replay read's own ownership check.
    for path_suffix in ["", "/events"] {
        let response = client
            .get(format!("{base}/api/recordings/rec-theirs{path_suffix}"))
            .header("cookie", cookie.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404, "{path_suffix}");
    }
    let signed_out = client
        .get(format!("{base}/api/recordings"))
        .send()
        .await
        .unwrap();
    assert_eq!(signed_out.status(), 401);

    // A cursor that does not parse is refused rather than read as "start
    // again", which would hand back the first page and look like a list that
    // repeats itself.
    let nonsense = client
        .get(format!("{base}/api/recordings?before=not-a-cursor"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(nonsense.status(), 400);
    assert_eq!(
        nonsense.json::<Value>().await.unwrap()["code"],
        "cursor_invalid"
    );

    server.abort();
    remove_database(path);
}

#[tokio::test]
async fn history_expired_returns_410() {
    let (base, server, path, client, cookie) = recorded_server("history-expired").await;
    let interview = start_interview(&client, &base, &cookie).await;
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at, expires_at
        ) VALUES ('rec-expired', 1, ?1, 'interview-abc12345', ?1,
            'one@example.test', 'ready', 10, 10, 1)
        ",
                [&interview],
            )
            .unwrap();
    }

    // Gone rather than missing: this account owns the interview and is owed the
    // difference between "never yours" and "not any more".
    for path_suffix in ["", "/events"] {
        let response = client
            .get(format!("{base}/api/recordings/rec-expired{path_suffix}"))
            .header("cookie", cookie.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 410, "{path_suffix}");
        assert_eq!(
            response.json::<Value>().await.unwrap()["code"],
            "replay_expired"
        );
    }

    // A deleted recording says so in its own words, because the two are
    // different things to a person: one waited too long, the other was taken
    // away.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "
        UPDATE recordings SET state = 'deleted', deleted_at = 5, expires_at = NULL,
            room_name = NULL, recipient_email = NULL WHERE id = 'rec-expired'
        ",
            [],
        )
        .unwrap();
    let deleted = client
        .get(format!("{base}/api/recordings/rec-expired"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 410);
    assert_eq!(
        deleted.json::<Value>().await.unwrap()["code"],
        "recording_deleted"
    );

    server.abort();
    remove_database(path);
}

/// A late join reads the snapshot, and only its own.
#[tokio::test]
async fn replay_snapshot_is_owner_scoped() {
    let (base, server, path, client, cookie) = recorded_server("replay-snapshot").await;
    let interview = start_interview(&client, &base, &cookie).await;
    let event = |kind: &str, text: &str| {
        json!({
            "v": codetrial::recording::REPLAY_VERSION,
            "kind": kind,
            "at": 1_770_000_000_000i64,
            "payload": { "text": text }
        })
    };
    let posted = client
        .post(format!("{base}/api/interviews/{interview}/events"))
        .header("cookie", cookie.clone())
        .json(&json!({ "events": [
            event("editor", "first draft"),
            event("transcript", "hello"),
            event("editor", "second draft"),
        ] }))
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), 200);

    let snapshot = client
        .get(format!("{base}/api/interviews/{interview}/snapshot"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(snapshot.status(), 200);
    let body = snapshot.json::<Value>().await.unwrap();
    assert_eq!(body["seq"], 2, "the last event the snapshot accounts for");
    assert_eq!(body["quotaExceeded"], false);
    assert_eq!(
        body["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| (
                event["seq"].as_i64().unwrap(),
                event["kind"].as_str().unwrap()
            ))
            .collect::<Vec<_>>(),
        vec![(1, "transcript"), (2, "editor")],
        "the superseded editor snapshot is dropped and the transcript line is not"
    );

    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "INSERT INTO users (id, github_id, login, email, email_verified, created_at, updated_at)
                   VALUES (2, 202, 'two', 'two@example.test', 1, 1, 1);
                 INSERT INTO interviews (id, account_id, consent_version, consent_at)
                   VALUES ('int-theirs', 2, '2026-08-21', 1);",
            )
            .unwrap();
    }
    let theirs = client
        .get(format!("{base}/api/interviews/int-theirs/snapshot"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(theirs.status(), 404);

    // Past the retention deadline the replay is gone rather than missing: this
    // account owns the interview and is owed the difference.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, expires_at, created_at, updated_at
        ) VALUES ('rec-snap', 1, ?1, 'interview-abc12345', ?1,
            'one@example.test', 'ready', 1, 1, 1)
        ",
            [&interview],
        )
        .unwrap();
    let expired = client
        .get(format!("{base}/api/interviews/{interview}/snapshot"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(expired.status(), 410);
    assert_eq!(
        expired.json::<Value>().await.unwrap()["code"],
        "replay_expired"
    );

    let signed_out = client
        .get(format!("{base}/api/interviews/{interview}/snapshot"))
        .send()
        .await
        .unwrap();
    assert_eq!(signed_out.status(), 401);

    server.abort();
    remove_database(path);
}

/// A signed-in account on a server that records, which is where every consent
/// test starts.
async fn recorded_server(
    label: &str,
) -> (
    String,
    tokio::task::JoinHandle<Result<(), std::io::Error>>,
    std::path::PathBuf,
    reqwest::Client,
    String,
) {
    let (mut config, cookie, path) = signed_in_web_config(label);
    config.recording = Some(recording_config());
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE users SET email = 'one@example.test', email_verified = 1 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    let (base, server) = spawn_web_server(config).await;
    (base, server, path, reqwest::Client::new(), cookie)
}

/// The same, with a provider a test can watch. Every failure the pipeline has
/// to survive is the provider's, and none can be produced against LiveKit on
/// demand.
async fn recorded_server_with_provider(
    label: &str,
    provider: std::sync::Arc<FakeRecordingProvider>,
) -> (
    String,
    tokio::task::JoinHandle<Result<(), std::io::Error>>,
    std::path::PathBuf,
    reqwest::Client,
    String,
) {
    let (mut config, cookie, path) = signed_in_web_config(label);
    config.recording = Some(recording_config());
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE users SET email = 'one@example.test', email_verified = 1 WHERE id = 1",
                [],
            )
            .unwrap();
    }
    let recorder = codetrial::recording::Recorder {
        provider,
        clock: std::sync::Arc::new(codetrial::recording::SystemClock),
        delivery: None,
        config: recording_config(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            codetrial::web::web_service_with_recorder(config, None, recorder),
        )
        .await
    });
    (
        format!("http://{addr}"),
        server,
        path,
        reqwest::Client::new(),
        cookie,
    )
}

/// A provider that records what it was asked and always agrees.
#[derive(Default)]
struct FakeRecordingProvider {
    started: std::sync::Mutex<Vec<String>>,
    stopped: std::sync::Mutex<Vec<String>>,
    /// A database to move the row in while `start` is in flight, standing in
    /// for a sweeper retry that won the race. This is the only way to reach the
    /// interleaving from a test: the handler reads its snapshot, the row
    /// changes, and then the handler answers.
    preempt: std::sync::Mutex<Option<std::path::PathBuf>>,
}

impl FakeRecordingProvider {
    fn starts(&self) -> usize {
        self.started.lock().unwrap().len()
    }

    fn stops(&self) -> Vec<String> {
        self.stopped.lock().unwrap().clone()
    }
}

impl codetrial::recording::RecordingProvider for FakeRecordingProvider {
    fn start<'a>(
        &'a self,
        request: &'a codetrial::recording::StartEgress,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
    {
        Box::pin(async move {
            self.started.lock().unwrap().push(request.room_name.clone());
            if let Some(path) = self.preempt.lock().unwrap().as_ref() {
                rusqlite::Connection::open(path)
                    .unwrap()
                    .execute(
                        "
                        UPDATE recordings
                        SET egress_id = 'EG_swept', state = 'recording'
                        WHERE room_name = ?1
                        ",
                        [&request.room_name],
                    )
                    .unwrap();
            }
            Ok("EG_web_fake".to_string())
        })
    }

    fn stop<'a>(
        &'a self,
        egress_id: &'a str,
        _room_name: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.stopped.lock().unwrap().push(egress_id.to_string());
            Ok(())
        })
    }

    fn active_for_room<'a>(
        &'a self,
        _room_name: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<String>, String>> + Send + 'a>,
    > {
        Box::pin(async move { Ok(None) })
    }
}

/// A second verified account, so cross-account refusals have somebody to be
/// refused for.
async fn second_account(
    client: &reqwest::Client,
    base: &str,
    path: &std::path::Path,
) -> (String, i64) {
    let cookie = record_login(client, base, "two").await;
    let connection = rusqlite::Connection::open(path).unwrap();
    let id: i64 = connection
        .query_row("SELECT id FROM users WHERE login = 'two'", [], |row| {
            row.get(0)
        })
        .unwrap();
    connection
        .execute(
            "UPDATE users SET email = 'two@example.test', email_verified = 1 WHERE id = ?1",
            [id],
        )
        .unwrap();
    (cookie, id)
}

fn interview_rows(path: &std::path::Path) -> Vec<(String, String, i64, Option<i64>)> {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare("SELECT id, consent_version, consent_at, consent_withdrawn_at FROM interviews")
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The quota stub without a hit counter, which is all most callers need.
async fn spawn_livekit_quota_stub(
    status: axum::http::StatusCode,
    delay: Duration,
) -> (String, tokio::task::JoinHandle<Result<(), std::io::Error>>) {
    let (url, _hits, server) = spawn_counting_quota_stub(status, delay).await;
    (url, server)
}

/// Like [`spawn_livekit_quota_stub`], but counts how many times the project was
/// asked. The count is the whole point of the test below: it is the difference
/// between a pool that probes on a candidate's request and one that already
/// knew.
async fn spawn_counting_quota_stub(
    status: axum::http::StatusCode,
    delay: Duration,
) -> (
    String,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<Result<(), std::io::Error>>,
) {
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = hits.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = axum::Router::new().route(
        "/rtc/validate",
        axum::routing::get(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tokio::time::sleep(delay).await;
                if status == axum::http::StatusCode::TOO_MANY_REQUESTS {
                    (
                        status,
                        "connection minutes limit exceeded. please contact the project owner.",
                    )
                } else {
                    (status, "success")
                }
            }
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, router).await });
    (format!("ws://127.0.0.1:{}", addr.port()), hits, server)
}

/// The pool probes itself, rather than making the first candidate of every
/// minute pay for the answer.
///
/// Before this, the quota verdict was learned lazily on `/api/token` and cached
/// for a minute, so one request in each window waited on an HTTP round trip per
/// project it had to consider. The background refresher keeps the cache warm,
/// which is observable here as a token request that adds no probe of its own.
#[tokio::test]
async fn the_pool_probes_in_the_background_so_a_token_request_does_not() {
    let (provider, hits, stub) =
        spawn_counting_quota_stub(axum::http::StatusCode::OK, Duration::ZERO).await;

    let (mut config, cookie, db_path) = signed_in_web_config("quota-background-probe");
    config.pool = primary_pool(&provider, "key", "secret");

    // The one test that wants the refresher, pointed at a local stub. Every
    // other server here leaves it off, so the suite makes no outbound request.
    config.probe_provider_quota = true;
    let (base, server) = spawn_web_server(config).await;

    // The startup pass is the refresher's first action, not a step that blocks
    // the bind, so it is raced against here rather than assumed complete.
    let mut probed = 0;
    for _ in 0..50 {
        probed = hits.load(std::sync::atomic::Ordering::Relaxed);
        if probed > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        probed > 0,
        "the pool must probe its projects without being asked for a token"
    );

    let response = reqwest::Client::new()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    assert_eq!(
        hits.load(std::sync::atomic::Ordering::Relaxed),
        probed,
        "the token request must be served from the warm cache, not from a fresh probe"
    );

    server.abort();
    stub.abort();
    remove_database(db_path);
}

/// A quota check must never make starting an interview wait for the shared
/// client's 30-second backstop when a provider accepts connections but stalls.
#[tokio::test]
async fn a_stalled_quota_probe_does_not_delay_token_issuance() {
    let (provider, stub) =
        spawn_livekit_quota_stub(axum::http::StatusCode::OK, Duration::from_secs(3)).await;
    let (mut config, cookie, db_path) = signed_in_web_config("quota-probe-timeout");
    config.pool = primary_pool(&provider, "available-key", "available-secret");
    let (base, server) = spawn_web_server(config).await;

    let started = std::time::Instant::now();
    let response = reqwest::Client::new()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a stalled quota probe must use its short timeout"
    );

    server.abort();
    stub.abort();
    remove_database(db_path);
}

/// A project out of connection minutes refuses the candidate's WebSocket, and a
/// browser cannot see the status on a refused upgrade: it surfaces as a bare
/// socket error, so the candidate is told only that the connection failed. The
/// pool exists precisely so one spent project is not the end of the interview,
/// so the server has to ask before it hands the token out.
#[tokio::test]
async fn a_provider_out_of_connection_minutes_is_passed_over() {
    let (exhausted, first) =
        spawn_livekit_quota_stub(axum::http::StatusCode::TOO_MANY_REQUESTS, Duration::ZERO).await;
    let (healthy, second) =
        spawn_livekit_quota_stub(axum::http::StatusCode::OK, Duration::ZERO).await;

    let (mut config, cookie, db_path) = signed_in_web_config("quota-failover");
    config.pool = codetrial::config::ProviderPool {
        providers: vec![
            codetrial::config::Provider {
                id: codetrial::config::PRIMARY_PROVIDER_ID.to_string(),
                url: exhausted.clone(),
                api_key: "spent-key".to_string(),
                api_secret: "spent-secret".to_string(),
                google_api_key: String::new(),
            },
            codetrial::config::Provider {
                id: "spare".to_string(),
                url: healthy.clone(),
                api_key: "spare-key".to_string(),
                api_secret: "spare-secret".to_string(),
                google_api_key: String::new(),
            },
        ],
    };
    let (base, server) = spawn_web_server(config).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.json::<Value>().await.unwrap();

    assert_eq!(
        body["serverUrl"], healthy,
        "the candidate must be sent to the project that still has minutes: {body}"
    );

    // The room name carries the provider id, which is how the agent resolves
    // the same project. Landing on the spare is not enough if the room still
    // says primary: the agent would join the project the candidate was moved
    // off.
    assert!(
        body["roomName"].as_str().unwrap().contains("-spare-"),
        "the room must name the project the candidate was moved to: {body}"
    );

    server.abort();
    first.abort();
    second.abort();
    remove_database(db_path);
}

/// A pinned `INTERVIEW_ROOM_NAME` is a convenience, not a promise the candidate
/// pays for. When the project it names is out of minutes the pin is dropped
/// rather than honoured: honouring it mints a token whose socket LiveKit
/// refuses with 429, which reaches the browser as "could not establish signal
/// connection" and sends the candidate looking at their own network.
///
/// The room name has to move with the project. It is the only thing a
/// separately launched `codetrial run-livekit ROOM` resolves the project from,
/// so a pinned name kept beside another project's credentials is how the
/// candidate and the agent end up in different ones.
#[tokio::test]
async fn a_pinned_room_on_an_exhausted_project_falls_back_to_the_pool() {
    let (exhausted, first) =
        spawn_livekit_quota_stub(axum::http::StatusCode::TOO_MANY_REQUESTS, Duration::ZERO).await;
    let (healthy, second) =
        spawn_livekit_quota_stub(axum::http::StatusCode::OK, Duration::ZERO).await;

    let (mut config, cookie, db_path) = signed_in_web_config("pinned-quota-failover");
    config.fixed_room_name = Some("interview-local".to_string());
    config.pool = codetrial::config::ProviderPool {
        providers: vec![
            codetrial::config::Provider {
                id: codetrial::config::PRIMARY_PROVIDER_ID.to_string(),
                url: exhausted.clone(),
                api_key: "spent-key".to_string(),
                api_secret: "spent-secret".to_string(),
                google_api_key: String::new(),
            },
            codetrial::config::Provider {
                id: "spare".to_string(),
                url: healthy.clone(),
                api_key: "spare-key".to_string(),
                api_secret: "spare-secret".to_string(),
                google_api_key: String::new(),
            },
        ],
    };
    let (base, server) = spawn_web_server(config).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.json::<Value>().await.unwrap();

    assert_eq!(
        body["serverUrl"], healthy,
        "an exhausted pin must not be handed to the candidate: {body}"
    );
    assert_ne!(
        body["roomName"], "interview-local",
        "the pinned name belongs to the exhausted project and must be dropped with it: {body}"
    );
    assert!(
        body["roomName"].as_str().unwrap().contains("-spare-"),
        "the room must name the project the candidate was moved to: {body}"
    );

    server.abort();
    first.abort();
    second.abort();
    remove_database(db_path);
}

/// A pinned room whose project still has minutes is left alone. The failover
/// above must not cost every local run its stable URL.
#[tokio::test]
async fn a_pinned_room_on_a_healthy_project_is_kept() {
    let (healthy, stub) =
        spawn_livekit_quota_stub(axum::http::StatusCode::OK, Duration::ZERO).await;

    let (mut config, cookie, db_path) = signed_in_web_config("pinned-quota-healthy");
    config.fixed_room_name = Some("interview-local".to_string());
    config.pool = primary_pool(&healthy, "key", "secret");
    let (base, server) = spawn_web_server(config).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.json::<Value>().await.unwrap();

    assert_eq!(
        body["roomName"], "interview-local",
        "a healthy pin is the whole point of INTERVIEW_ROOM_NAME: {body}"
    );
    assert_eq!(body["serverUrl"], healthy);

    server.abort();
    stub.abort();
    remove_database(db_path);
}

/// With nothing left to fail over to, the honest answer names the cause. The
/// browser cannot: a refused upgrade reaches it as an unexplained socket error,
/// so a candidate would go looking at their own network for a quota this server
/// already knows is spent.
#[tokio::test]
async fn every_provider_out_of_minutes_is_refused_with_the_reason() {
    let (exhausted, stub) =
        spawn_livekit_quota_stub(axum::http::StatusCode::TOO_MANY_REQUESTS, Duration::ZERO).await;

    let (mut config, cookie, db_path) = signed_in_web_config("quota-exhausted");
    config.pool = primary_pool(&exhausted, "spent-key", "spent-secret");
    let (base, server) = spawn_web_server(config).await;

    let response = reqwest::Client::new()
        .post(format!("{base}/api/token"))
        .header("cookie", &cookie)
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    let body = response.json::<Value>().await.unwrap();
    assert_eq!(body["code"], "livekit_quota_exhausted", "{body}");

    server.abort();
    stub.abort();
    remove_database(db_path);
}

/// A dead project must not skew the pool. It still consumes its turn in the
/// rotation, so the project after it serves its own share and no more.
///
/// The failure this pins is not a refusal but a lopsided one: taking a single
/// rotation step per request and adding the retry offset locally leaves every
/// request starting at the dead project again, so its neighbour answers twice
/// as often as the rest. With three projects and one spent, six starts must
/// split three and three, not four and two.
#[tokio::test]
async fn a_dead_project_does_not_skew_the_rotation() {
    let (spent, spent_stub) =
        spawn_livekit_quota_stub(axum::http::StatusCode::TOO_MANY_REQUESTS, Duration::ZERO).await;
    let (first, first_stub) =
        spawn_livekit_quota_stub(axum::http::StatusCode::OK, Duration::ZERO).await;
    let (second, second_stub) =
        spawn_livekit_quota_stub(axum::http::StatusCode::OK, Duration::ZERO).await;

    let (mut config, cookie, db_path) = signed_in_web_config("quota-rotation");
    config.pool = codetrial::config::ProviderPool {
        providers: vec![
            codetrial::config::Provider {
                id: codetrial::config::PRIMARY_PROVIDER_ID.to_string(),
                url: spent.clone(),
                api_key: "spent-key".to_string(),
                api_secret: "spent-secret".to_string(),
                google_api_key: String::new(),
            },
            codetrial::config::Provider {
                id: "first".to_string(),
                url: first.clone(),
                api_key: "first-key".to_string(),
                api_secret: "first-secret".to_string(),
                google_api_key: String::new(),
            },
            codetrial::config::Provider {
                id: "second".to_string(),
                url: second.clone(),
                api_key: "second-key".to_string(),
                api_secret: "second-secret".to_string(),
                google_api_key: String::new(),
            },
        ],
    };
    let (base, server) = spawn_web_server(config).await;

    let client = reqwest::Client::new();
    let mut served = std::collections::HashMap::new();
    for _ in 0..6 {
        let body = client
            .post(format!("{base}/api/token"))
            .header("cookie", &cookie)
            .header("content-type", "application/json")
            .body("{}")
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap();
        let url = body["serverUrl"].as_str().unwrap().to_string();
        *served.entry(url).or_insert(0) += 1;
    }

    assert_eq!(
        served.get(&spent),
        None,
        "a spent project must serve nobody"
    );
    assert_eq!(served.get(&first), Some(&3), "{served:?}");
    assert_eq!(served.get(&second), Some(&3), "{served:?}");

    server.abort();
    spent_stub.abort();
    first_stub.abort();
    second_stub.abort();
    remove_database(db_path);
}

/// Ending an interview that recorded nothing says so, rather than failing.
///
/// 204 and no body: the interview ending is the candidate's news whether or
/// not a recording was running, so there is nothing to report and nothing to
/// stop. A 404 would tell them the interview was never theirs.
#[tokio::test]
async fn ending_an_interview_without_a_recording_is_a_no_op() {
    let (base, server, path, client, cookie) = recorded_server("end-no-recording").await;
    let interview = start_interview(&client, &base, &cookie).await;

    let ended = client
        .post(format!("{base}/api/interviews/{interview}/end"))
        .header("cookie", cookie.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(ended.status(), 204);
    assert!(ended.bytes().await.unwrap().is_empty());

    // Idempotent, because the browser sends this on unload and on the button.
    let again = client
        .post(format!("{base}/api/interviews/{interview}/end"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 204);

    server.abort();
    remove_database(path);
}

/// Every route that acts on an account rejects a request carrying no session.
///
/// These handlers take an `Owner` extractor rather than checking inline. The
/// compiler proves the extractor runs; this proves what it answers when nobody
/// is signed in, for every owner-scoped route rather than the five that had
/// coverage scattered through this file.
///
/// The second half keeps the first half honest. The table is hand-written, so
/// a thirteenth owner-scoped route would otherwise join nothing: instead every
/// route `declared_routes` finds has to appear either here or in the anonymous
/// list, and a new one fails until someone says which it is.
#[tokio::test]
async fn every_owner_scoped_route_refuses_an_anonymous_request() {
    let (base, server, path, client, _cookie) = recorded_server("anon-owner-routes").await;
    let owner_scoped = [
        (reqwest::Method::GET, "/api/reports"),
        (reqwest::Method::POST, "/api/reports"),
        (reqwest::Method::POST, "/api/interviews"),
        (reqwest::Method::DELETE, "/api/interviews/{id}/consent"),
        (reqwest::Method::POST, "/api/interviews/{id}/recording"),
        (reqwest::Method::GET, "/api/interviews/{id}/recording"),
        (reqwest::Method::POST, "/api/interviews/{id}/end"),
        (reqwest::Method::POST, "/api/interviews/{id}/events"),
        (reqwest::Method::GET, "/api/interviews/{id}/snapshot"),
        (reqwest::Method::GET, "/api/recordings"),
        (reqwest::Method::GET, "/api/recordings/{id}"),
        (reqwest::Method::GET, "/api/recordings/{id}/events"),
    ];

    for (method, route) in owner_scoped.clone() {
        let target = route.replace("{id}", "some-interview");
        let status = client
            .request(method.clone(), format!("{base}{target}"))
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, 401, "{method} {target} answered {status}, not 401");
    }

    // Routes that answer without a session. The two LiveKit paths belong here
    // because they authenticate a signed machine principal against a room
    // rather than a person against a cookie.
    let anonymous = [
        "/healthz",
        "/api/token",
        "/api/observer-token",
        "/api/login",
        "/api/callback",
        "/api/session",
        "/api/logout",
        "/runtime-config.js",
        codetrial::recording::WEBHOOK_ROUTE,
        codetrial::recording::REPLAY_ROUTE,
    ];
    for route in declared_routes() {
        let known = owner_scoped.iter().any(|(_, path)| *path == route)
            || anonymous.contains(&route.as_str());
        assert!(
            known,
            "{route} is in the router but is classified neither owner-scoped nor \
             anonymous; add it to one of the two lists in this test"
        );
    }

    server.abort();
    remove_database(path);
}

/// `Provider` and `RecordingConfig` both hand-write a redacting `Debug`, and
/// both name `WebServerConfig` as the reason: it derives `Debug`, so anything
/// reachable from it can reach a log line. The two secrets the struct holds
/// directly were the ones nobody checked, and the derive printed both in full.
///
/// `session_secret` is why this is a test rather than a style note. It signs
/// the session cookie, so a copy on stderr is not a credential to steal but the
/// ability to mint any user's session.
#[test]
fn web_server_config_debug_redacts_its_own_secrets() {
    let mut config = web_config();
    config.github_client_id = Some("public-client-id".to_string());
    config.github_client_secret = Some("CLIENTSECRET-def456".to_string());
    config.session_secret = Some("SESSIONSECRET-abc123".to_string());

    let printed = format!("{config:?}");
    for secret in ["CLIENTSECRET-def456", "SESSIONSECRET-abc123"] {
        assert!(
            !printed.contains(secret),
            "Debug output leaked {secret}: {printed}"
        );
    }
    assert!(
        printed.contains("<redacted>"),
        "the redaction should be visible rather than silent: {printed}"
    );

    // The pool redacts one level down, which is what makes the whole struct
    // safe to print rather than just its top level.
    assert!(
        !printed.contains("devsecret"),
        "the provider secret leaked through the pool: {printed}"
    );

    // Absent stays absent: `None` must not print as a redacted value, or an
    // operator reading a log cannot tell a configured secret from a missing
    // one.
    let blank = format!("{:?}", web_config());
    assert!(
        blank.contains("session_secret: None"),
        "an unset secret should read as None: {blank}"
    );

    // The client id is not a secret, and redacting it would cost the one field
    // that makes an OAuth misconfiguration diagnosable from a log.
    assert!(
        printed.contains("public-client-id"),
        "the client id is public and should stay readable: {printed}"
    );
}

/// The two stores must answer the same URL the same way.
///
/// What this catches is a re-split: both stores now resolve through one
/// candidate list, and if somebody gives either of them its own rules again,
/// this fails. Verified by doing exactly that, not assumed.
///
/// What it cannot catch is a wrong rule, because a shared mistake stays
/// symmetric and parity still holds. Behavior of the rules themselves belongs
/// in the per-store tests above; this one owns the contract between them.
///
/// Status, content type, and cache policy only. ETags are deliberately
/// different: the disk side is a modification-time heuristic and the embedded
/// side is a digest of bytes that cannot change without a rebuild.
#[tokio::test]
async fn disk_and_embedded_stores_resolve_urls_identically() {
    let mut disk = web_config();
    disk.web_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("web");
    let (disk_base, disk_server) = spawn_web_server(disk).await;

    let mut embedded = web_config();
    embedded.web_dir = absent_web_dir("resolver-parity");
    let (embedded_base, embedded_server) = spawn_web_server(embedded).await;

    for route in [
        "/",
        "/interview",
        "/interview/",
        "/index.html",
        "/vendor/avatar/three-vrm.js",
        "/vendor/avatar/three-vrm.js/",
        "/missing.js",
        "/missing/deeper.css",
        "/.git/config",
        "/vendor/.hidden",
    ] {
        let from_disk = reqwest::get(&format!("{disk_base}{route}")).await.unwrap();
        let from_embed = reqwest::get(&format!("{embedded_base}{route}"))
            .await
            .unwrap();
        assert_eq!(
            from_disk.status(),
            from_embed.status(),
            "status differs for {route}"
        );
        assert_eq!(
            from_disk.headers().get("content-type"),
            from_embed.headers().get("content-type"),
            "content type differs for {route}"
        );
        assert_eq!(
            from_disk.headers().get("cache-control"),
            from_embed.headers().get("cache-control"),
            "cache policy differs for {route}"
        );
    }

    disk_server.abort();
    embedded_server.abort();
}

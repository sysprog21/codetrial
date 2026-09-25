//! Serving web/: from disk, from the embedded copy, and the traversal it
//! refuses.
//!
//! Split out of `tests/web.rs`, which is still the test target: cargo
//! discovers only `tests/*.rs`, so this compiles as a module of that one
//! binary rather than relinking the crate for a file of its own.

use super::*;

/// Every refused name is a file that is really there.
///
/// This asked `src/web` for `/.env.local` and `/../agent/.env`, neither of
/// which exists, so `None` was the answer whether the resolver refused the path
/// or merely failed to find it: deleting the whole refusal from
/// `static_candidates` left this passing. The rule itself is covered by the
/// unit tests beside it in `src/web/assets.rs`; what only a resolver test can
/// say is that `static_file_meta` honours the refusal against a file it would
/// otherwise hand back, which is the only case where the difference is visible.
///
/// So every name below is created first. A trailing dot and an embedded
/// backslash are ordinary filename bytes here and directory syntax on Windows,
/// which is why they are refused and why they can be created to ask.
#[tokio::test]
async fn static_file_rejects_traversal_and_dotfiles() {
    let base = unique_temp_path("refused-segments", "");
    let root = base.join("root");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::create_dir_all(base.join("outside")).unwrap();
    fs::write(base.join("outside/secret.txt"), "not yours").unwrap();
    fs::write(root.join("app.js"), "ok").unwrap();
    fs::write(root.join(".env.local"), "not yours").unwrap();
    fs::write(root.join("sub/.env"), "not yours").unwrap();
    fs::write(root.join("trailing."), "not yours").unwrap();
    fs::write(root.join("back\\slash.js"), "not yours").unwrap();

    // The control, and the thing that makes the refusals below mean something:
    // this resolver does find files in this tree.
    assert_eq!(
        resolved(&root, "/app.js").await,
        Some(root.join("app.js")),
        "the resolver has to be able to answer at all"
    );

    for refused in [
        "/.env.local",
        "/sub/.env",
        "/trailing.",
        "/back\\slash.js",
        "/../outside/secret.txt",
        "/sub/../.env.local",
    ] {
        assert!(
            resolved(&root, refused).await.is_none(),
            "{refused} names a file that exists, and must still be refused"
        );
    }

    fs::remove_dir_all(base).unwrap();
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
    // Whichever deadline this process's environment selects: a run with
    // CODETRIAL_GEMINI_REST_BASE set is told the local wait, and that is right.
    // Without it the hosted wait is pinned exactly, which is also what catches
    // a `report_endpoint_is_local` that answers true for Google.
    let escape_wait_ms =
        codetrial::livekit::report_escape_wait(codetrial::livekit::report_timeout()).as_millis();
    let base_set =
        std::env::var("CODETRIAL_GEMINI_REST_BASE").is_ok_and(|base| !base.trim().is_empty());
    if base_set {
        assert!(
            matches!(escape_wait_ms, 135_000 | 250_000),
            "{escape_wait_ms}"
        );
    } else {
        assert_eq!(escape_wait_ms, 135_000);
    }
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
        format!(
            "globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = true;\nglobalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = \"https://godbolt.org\";\nglobalThis.CODETRIAL_RECORDING_ENABLED = false;\nglobalThis.CODETRIAL_CONSENT_VERSION = \"2026-08-21\";\nglobalThis.CODETRIAL_REPLAY_VERSION = 1;\nglobalThis.CODETRIAL_REPORT_ESCAPE_WAIT_MS = {escape_wait_ms};\n"
        )
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
        format!(
            "globalThis.CODETRIAL_COMPILER_EXPLORER_ENABLED = false;\nglobalThis.CODETRIAL_COMPILER_EXPLORER_BASE_URL = \"\";\nglobalThis.CODETRIAL_RECORDING_ENABLED = false;\nglobalThis.CODETRIAL_CONSENT_VERSION = \"2026-08-21\";\nglobalThis.CODETRIAL_REPLAY_VERSION = 1;\nglobalThis.CODETRIAL_REPORT_ESCAPE_WAIT_MS = {escape_wait_ms};\n"
        )
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

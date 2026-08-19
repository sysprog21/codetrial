use codetrial::config::{
    DEFAULT_COMPILER_EXPLORER_ENABLED, DEFAULT_DURATION_MIN,
    DEFAULT_GEMINI_CANDIDATE_VIDEO_ENABLED, DEFAULT_GEMINI_LIVE_MODEL, DEFAULT_GEMINI_REPORT_MODEL,
    DEFAULT_GEMINI_VOICE, DEFAULT_ROOM_PREFIX, DEFAULT_WEB_ADDR, DEFAULT_WEB_DIR, load_from_pairs,
};
use serde_json::Value;

#[test]
fn config_accepts_current_env_names() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
        ("GEMINI_LIVE_MODEL", "live-model"),
        ("GEMINI_REPORT_MODEL", "report-model"),
        ("GEMINI_VOICE", "Voice"),
        ("CODETRIAL_ROOM_PREFIX", "room"),
        ("CODETRIAL_DURATION_MIN", "30"),
        ("CODETRIAL_WEB_DIR", "public"),
        ("CODETRIAL_WEB_ADDR", "0.0.0.0:8080"),
        ("CODETRIAL_COMPILER_EXPLORER_ENABLED", "false"),
        ("CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED", "true"),
    ])
    .expect("complete config should load");

    assert_eq!(config.livekit_url, "wss://example.livekit.cloud");
    assert_eq!(config.livekit_api_key, "key");
    assert_eq!(config.livekit_api_secret, "secret");
    assert_eq!(config.google_api_key, "google");
    assert_eq!(config.gemini_live_model, "live-model");
    assert_eq!(config.gemini_report_model, "report-model");
    assert_eq!(config.gemini_voice, "Voice");
    assert_eq!(config.room_prefix, "room");
    assert_eq!(config.default_duration_min, 30);
    assert_eq!(config.web_dir, "public");
    assert_eq!(config.web_addr, "0.0.0.0:8080");
    assert!(!config.compiler_explorer_enabled);
    assert!(config.gemini_candidate_video_enabled);
}

#[test]
fn config_defaults_optional_names_and_web_runtime_fields() {
    let expected: Value = serde_json::from_str(include_str!("golden/gemini-defaults.json"))
        .expect("Gemini defaults fixture should parse");
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .expect("required-only config should load");

    assert_eq!(config.gemini_live_model, DEFAULT_GEMINI_LIVE_MODEL);
    assert_eq!(config.gemini_report_model, DEFAULT_GEMINI_REPORT_MODEL);
    assert_eq!(config.gemini_voice, DEFAULT_GEMINI_VOICE);
    assert_eq!(config.room_prefix, DEFAULT_ROOM_PREFIX);
    assert_eq!(config.default_duration_min, DEFAULT_DURATION_MIN);
    assert_eq!(config.web_dir, DEFAULT_WEB_DIR);
    assert_eq!(config.web_addr, DEFAULT_WEB_ADDR);
    assert_eq!(
        config.compiler_explorer_enabled,
        DEFAULT_COMPILER_EXPLORER_ENABLED
    );
    assert_eq!(
        config.gemini_candidate_video_enabled,
        DEFAULT_GEMINI_CANDIDATE_VIDEO_ENABLED
    );
    let rust = serde_json::json!([
        DEFAULT_GEMINI_LIVE_MODEL,
        DEFAULT_GEMINI_REPORT_MODEL,
        DEFAULT_GEMINI_VOICE,
    ]);
    assert_eq!(rust, expected);
}

#[test]
fn config_uses_defaults_for_blank_optional_web_runtime_fields() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
        ("CODETRIAL_ROOM_PREFIX", " "),
        ("CODETRIAL_DURATION_MIN", " "),
        ("CODETRIAL_WEB_DIR", " "),
        ("CODETRIAL_WEB_ADDR", " "),
        ("CODETRIAL_COMPILER_EXPLORER_ENABLED", " "),
        ("CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED", " "),
    ])
    .expect("blank optional config should load with defaults");

    assert_eq!(config.room_prefix, DEFAULT_ROOM_PREFIX);
    assert_eq!(config.default_duration_min, DEFAULT_DURATION_MIN);
    assert_eq!(config.web_dir, DEFAULT_WEB_DIR);
    assert_eq!(config.web_addr, DEFAULT_WEB_ADDR);
    assert_eq!(
        config.compiler_explorer_enabled,
        DEFAULT_COMPILER_EXPLORER_ENABLED
    );
    assert_eq!(
        config.gemini_candidate_video_enabled,
        DEFAULT_GEMINI_CANDIDATE_VIDEO_ENABLED
    );
}

#[test]
fn config_fails_closed_for_invalid_compiler_explorer_switch() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
        ("CODETRIAL_COMPILER_EXPLORER_ENABLED", "flase"),
    ])
    .expect("invalid privacy switch should still load");

    assert!(!config.compiler_explorer_enabled);
}

#[test]
fn config_fails_closed_for_invalid_gemini_candidate_video_switch() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
        ("CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED", "invalid_value"),
    ])
    .expect("invalid video switch should still load");

    assert!(!config.gemini_candidate_video_enabled);
}

#[test]
fn config_migrates_retired_report_model_aliases() {
    for retired in [
        "gemini-flash-latest",
        "gemini-2.5-flash",
        "models/gemini-2.5-flash",
    ] {
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "key"),
            ("LIVEKIT_API_SECRET", "secret"),
            ("GOOGLE_API_KEY", "google"),
            ("GEMINI_REPORT_MODEL", retired),
        ])
        .expect("retired report model alias should migrate");

        assert_eq!(config.gemini_report_model, DEFAULT_GEMINI_REPORT_MODEL);
    }
}

#[test]
fn config_rejects_missing_required_keys() {
    // Not `expect_err`, which would need `AgentConfig` to be printable, and it
    // holds two secrets in plain fields.
    let Err(error) = load_from_pairs([
        ("LIVEKIT_URL", " "),
        ("LIVEKIT_API_KEY", "key"),
        ("GEMINI_LIVE_MODEL", "live-model"),
    ]) else {
        panic!("missing required config should fail closed");
    };

    assert_eq!(
        error.missing_keys,
        vec!["LIVEKIT_URL", "LIVEKIT_API_SECRET", "GOOGLE_API_KEY"]
    );
    assert_eq!(
        error.to_string(),
        "missing required config keys: LIVEKIT_URL, LIVEKIT_API_SECRET, GOOGLE_API_KEY"
    );
}

/// Discovery takes a directory rather than reading the current one, so a test
/// can state exactly what is on disk. That is also the point of the argument:
/// two processes started from two directories used to build two different
/// pools.
fn provider_fixture(label: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("codetrial-providers-{label}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, contents) in files {
        std::fs::write(dir.join(name), contents).unwrap();
    }
    dir
}

fn provider_file(url: &str, key: &str, secret: &str) -> String {
    format!(
        "# a comment\n\nLIVEKIT_URL={url}\nLIVEKIT_API_KEY=\"{key}\"\nLIVEKIT_API_SECRET='{secret}'\n"
    )
}

#[test]
fn providers_are_discovered_in_a_stable_order() {
    let dir = provider_fixture(
        "stable-order",
        &[
            // Deliberately not alphabetical on disk. `read_dir` order is
            // unspecified, and a pool assembled in a different order sends the
            // candidate and the agent to different LiveKit projects.
            (
                "codetrial.env.tokyo",
                &provider_file("wss://tokyo.example", "k3", "s3"),
            ),
            (
                "codetrial.env.eu",
                &provider_file("wss://eu.example", "k1", "s1"),
            ),
            (
                "codetrial.env.sydney",
                &provider_file("wss://sydney.example", "k2", "s2"),
            ),
            // Neither the operator's live config nor the checked-in template is
            // a secondary provider.
            (
                "codetrial.env.local",
                &provider_file("wss://local.example", "k0", "s0"),
            ),
            (
                "codetrial.env.example",
                &provider_file("wss://example.example", "k0", "s0"),
            ),
            ("unrelated.txt", "LIVEKIT_URL=wss://nope.example\n"),
        ],
    );

    let (providers, warnings) = codetrial::config::discover_providers(&dir, false);

    assert_eq!(
        providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        vec!["eu", "sydney", "tokyo"]
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    // Quotes stripped, comments and blank lines ignored.
    assert_eq!(providers[0].url, "wss://eu.example");
    assert_eq!(providers[0].api_key, "k1");
    assert_eq!(providers[0].api_secret, "s1");
}

#[test]
fn a_broken_provider_file_is_skipped_rather_than_fatal() {
    let dir = provider_fixture(
        "broken-file",
        &[
            (
                "codetrial.env.good",
                &provider_file("wss://good.example", "k", "s"),
            ),
            // No secret: half a provider is not a provider.
            (
                "codetrial.env.partial",
                "LIVEKIT_URL=wss://partial.example\nLIVEKIT_API_KEY=k\n",
            ),
            // Not a URL this server knows how to reach.
            (
                "codetrial.env.scheme",
                &provider_file("ftp://bad.example", "k", "s"),
            ),
            // Scheme with nothing after it.
            ("codetrial.env.hostless", &provider_file("wss://", "k", "s")),
            ("codetrial.env.garbage", "this line has no equals sign\n"),
        ],
    );

    let (providers, warnings) = codetrial::config::discover_providers(&dir, false);

    // An optional extra project must not be able to stop the primary from
    // serving, so the good one still loads and the rest are reported.
    assert_eq!(
        providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        vec!["good"]
    );
    assert_eq!(warnings.len(), 4, "{warnings:?}");

    // Silently dropping one is what made a typo shrink the pool on one host and
    // not the other, so each has to name the file it came from.
    for id in ["partial", "scheme", "hostless", "garbage"] {
        assert!(
            warnings.iter().any(|warning| warning.contains(id)),
            "no warning names {id}: {warnings:?}"
        );
    }
}

/// Silence here is the failure mode the whole feature is trying to avoid: an
/// operator who believes a project is in the pool, and rooms that never reach
/// it. Anything named like a provider file that will not be loaded says so.
#[test]
fn a_file_that_looks_like_a_provider_but_cannot_be_one_says_so() {
    let dir = provider_fixture(
        "unusable-ids",
        &[
            (
                "codetrial.env.eu",
                &provider_file("wss://eu.example", "k", "s"),
            ),
            // Uppercase and dashes cannot survive a room name. The name has no
            // lowercase twin on purpose: a case-insensitive filesystem would
            // otherwise fold the two fixtures into one file.
            (
                "codetrial.env.ASIA",
                &provider_file("wss://upper.example", "k", "s"),
            ),
            (
                "codetrial.env.eu-west",
                &provider_file("wss://dashed.example", "k", "s"),
            ),
            // Taken by the credentials from the environment.
            (
                "codetrial.env.primary",
                &provider_file("wss://clash.example", "k", "s"),
            ),
            // Expected to be here, and not a provider.
            (
                "codetrial.env.local",
                &provider_file("wss://local.example", "k", "s"),
            ),
            (
                "codetrial.env.example",
                &provider_file("wss://tmpl.example", "k", "s"),
            ),
        ],
    );

    // A symlink is never followed, so it has to be reported rather than
    // dropped.
    std::os::unix::fs::symlink("/etc/passwd", dir.join("codetrial.env.link")).unwrap();

    let (providers, warnings) = codetrial::config::discover_providers(&dir, false);

    assert_eq!(
        providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        vec!["eu"]
    );
    for named in ["ASIA", "eu-west", "primary", "link"] {
        assert!(
            warnings.iter().any(|warning| warning.contains(named)),
            "no warning names {named}: {warnings:?}"
        );
    }
    // `local` and `example` belong here, so they are not noise.
    assert_eq!(warnings.len(), 4, "{warnings:?}");
    assert!(
        !warnings.iter().any(|warning| warning.contains("root:")),
        "a symlink must not be read: {warnings:?}"
    );
}

#[test]
fn plaintext_livekit_is_local_only() {
    for url in ["ws://127.0.0.1:7880", "http://127.0.0.1:7880"] {
        assert!(codetrial::config::validate_livekit_url(url, false).is_ok());
        // A join token is a bearer credential.
        assert!(codetrial::config::validate_livekit_url(url, true).is_err());
    }
    for url in [
        "wss://example.livekit.cloud",
        "https://example.livekit.cloud",
    ] {
        assert!(codetrial::config::validate_livekit_url(url, true).is_ok());
    }
    // Prefix matching alone let all of these through.
    for url in [
        "wss://",
        "https://",
        "example.livekit.cloud",
        "ftp://x.example",
    ] {
        assert!(
            codetrial::config::validate_livekit_url(url, false).is_err(),
            "{url} should not validate"
        );
    }
}

#[test]
fn a_livekit_error_never_quotes_the_url() {
    // The message reaches stderr, and a LiveKit URL can carry a query string.
    let error =
        codetrial::config::validate_livekit_url("ftp://host/?token=sensitive", false).unwrap_err();
    assert!(!error.contains("sensitive"), "{error}");
}

#[test]
fn a_room_name_names_the_provider_that_signed_its_token() {
    use codetrial::config::{PRIMARY_PROVIDER_ID, provider_id_from_room};

    // What `room_and_provider` mints for a secondary, and what the agent has to
    // read back out of it.
    assert_eq!(
        provider_id_from_room("interview-eu-a1b2c3d4", "interview"),
        Some("eu")
    );

    // The primary contributes no segment, so single-provider deployments keep
    // the room names they already had.
    assert_eq!(
        provider_id_from_room("interview-a1b2c3d4", "interview"),
        None
    );
    assert_eq!(provider_id_from_room("interview-local", "interview"), None);
    assert_eq!(
        provider_id_from_room("other-eu-a1b2c3d4", "interview"),
        None
    );
    assert_ne!(PRIMARY_PROVIDER_ID, "");
}

#[test]
fn an_empty_pool_selects_nothing() {
    let pool = codetrial::config::ProviderPool::default();
    assert!(pool.primary().is_none());
    assert!(pool.get("eu").is_none());
    // Modulo against an empty vector would panic.
    assert!(pool.select(7).is_none());
}

#[test]
fn the_pool_round_robins_and_looks_up_by_id() {
    let dir = provider_fixture(
        "selection",
        &[
            (
                "codetrial.env.eu",
                &provider_file("wss://eu.example", "k1", "s1"),
            ),
            (
                "codetrial.env.us",
                &provider_file("wss://us.example", "k2", "s2"),
            ),
        ],
    );
    let mut pool = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap()
    .pool;
    pool.providers
        .extend(codetrial::config::discover_providers(&dir, false).0);

    assert_eq!(pool.primary().unwrap().id, "primary");
    assert_eq!(pool.select(0).unwrap().id, "primary");
    assert_eq!(pool.select(1).unwrap().id, "eu");
    assert_eq!(pool.select(2).unwrap().id, "us");
    assert_eq!(pool.select(3).unwrap().id, "primary");

    assert_eq!(pool.get("us").unwrap().url, "wss://us.example");
    assert!(pool.get("nowhere").is_none());
}

#[test]
fn a_provider_debug_does_not_print_its_secret() {
    let dir = provider_fixture(
        "redaction",
        &[(
            "codetrial.env.eu",
            "LIVEKIT_URL=wss://eu.example\nLIVEKIT_API_KEY=livekit-key\nLIVEKIT_API_SECRET=hunter2\nGOOGLE_API_KEY=gsecret\n",
        )],
    );
    let (providers, _) = codetrial::config::discover_providers(&dir, false);

    let rendered = format!("{:?}", providers[0]);
    assert!(!rendered.contains("livekit-key"), "{rendered}");
    assert!(!rendered.contains("hunter2"), "{rendered}");
    assert!(!rendered.contains("gsecret"), "{rendered}");

    // The id and URL are what makes the line useful, and neither is a
    // credential.
    assert!(rendered.contains("eu"), "{rendered}");
}

#[test]
fn loading_config_does_not_depend_on_the_current_directory() {
    // `load_from_pairs` used to scan `config/` relative to the process working
    // directory, which made every caller, including these tests, non-hermetic.
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
    ])
    .unwrap();

    assert_eq!(config.pool.providers.len(), 1);
    assert_eq!(config.pool.providers[0].id, "primary");
    assert_eq!(config.pool.providers[0].google_api_key, "google");
}

#[test]
fn a_hostname_web_address_still_loads() {
    // `ToSocketAddrs` resolves this and `bind_web_listener` accepts it, so a
    // config-time check that only takes a literal socket address would refuse a
    // setting that has always worked.
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
        ("CODETRIAL_WEB_ADDR", "localhost:3000"),
    ])
    .expect("a hostname web address should load");

    assert_eq!(config.web_addr, "localhost:3000");
}

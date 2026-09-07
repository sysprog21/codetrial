use codetrial::config::{
    DEFAULT_COMPILER_EXPLORER_ENABLED, DEFAULT_DURATION_MIN,
    DEFAULT_GEMINI_CANDIDATE_VIDEO_ENABLED, DEFAULT_GEMINI_LIVE_MODEL, DEFAULT_GEMINI_REPORT_MODEL,
    DEFAULT_GEMINI_SILENCE_MS, DEFAULT_GEMINI_START_SENSITIVITY, DEFAULT_GEMINI_VOICE,
    DEFAULT_MAX_CONCURRENT_INTERVIEWS, DEFAULT_ROOM_PREFIX, MAX_GEMINI_SILENCE_MS, load_from_pairs,
    max_concurrent_interviews,
};
use serde_json::Value;
use std::collections::BTreeMap;

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
    assert_eq!(config.gemini_silence_ms, DEFAULT_GEMINI_SILENCE_MS);
    assert_eq!(config.room_prefix, "room");
    assert_eq!(config.default_duration_min, 30);
    assert!(config.gemini_candidate_video_enabled);
}

#[test]
fn config_caps_gemini_silence_at_the_point_it_stops_being_a_preference() {
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
        ("GEMINI_SILENCE_MS", "99999"),
    ])
    .expect("complete config should load");

    // Against the constant, not a literal. The two used to disagree about what
    // the cap even was, and the test name asserted a Gemini API limit that does
    // not exist: setup accepts 2001, 5000 and 30000 against the live endpoint.
    assert_eq!(config.gemini_silence_ms, MAX_GEMINI_SILENCE_MS);
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

    // The literal, not the constant. Which value the default holds is the thing
    // under test, and comparing it against itself passes however it is set.
    // `LOW` is the cautious one: `HIGH` is what cuts the interviewer off
    // mid-sentence, and flipping the default there has to fail here.
    assert_eq!(config.gemini_start_sensitivity, "START_SENSITIVITY_LOW");
    assert_eq!(config.room_prefix, DEFAULT_ROOM_PREFIX);
    assert_eq!(config.default_duration_min, DEFAULT_DURATION_MIN);
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
    assert_eq!(
        config.gemini_candidate_video_enabled,
        DEFAULT_GEMINI_CANDIDATE_VIDEO_ENABLED
    );
}

/// Asserted on the parser rather than through a config struct, because the
/// struct field it used to be read through was only ever consumed by `serve`.
///
/// The distinction worth pinning is blank against unreadable. This switch sends
/// candidate code to a third party, so a value nobody can read has to mean off,
/// while a blank one is an operator who said nothing and gets the default. A
/// parser that collapsed the two would either turn the feature off for an empty
/// line in a config file or leave it on for a typo.
#[test]
fn config_fails_closed_for_invalid_compiler_explorer_switch() {
    let enabled = codetrial::config::compiler_explorer_enabled;

    for unreadable in ["flase", "maybe", "2"] {
        assert!(!enabled(Some(unreadable)), "{unreadable} should mean off");
    }
    for said_nothing in [Some(""), Some("   "), None] {
        assert_eq!(
            enabled(said_nothing),
            DEFAULT_COMPILER_EXPLORER_ENABLED,
            "{said_nothing:?} should mean the default"
        );
    }
    for on in ["true", "TRUE", "1", "yes", "on"] {
        assert!(enabled(Some(on)), "{on} should mean on");
    }
    for off in ["false", "0", "no", "off"] {
        assert!(!enabled(Some(off)), "{off} should mean off");
    }
}

/// Both directions of the candidate-video switch, because one of them proves
/// nothing on its own.
///
/// `DEFAULT_GEMINI_CANDIDATE_VIDEO_ENABLED` is already `false`, so asserting
/// that an unreadable value reads as off is true whether the parser fails
/// closed or ignores the key entirely: this test used to pass against a build
/// that never looked at the environment at all. The `true` case is what
/// separates the two, and it has to be here for the `invalid_value` case below
/// to mean what its name says.
#[test]
fn config_fails_closed_for_invalid_gemini_candidate_video_switch() {
    let video = |value: &str| {
        load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "key"),
            ("LIVEKIT_API_SECRET", "secret"),
            ("GOOGLE_API_KEY", "google"),
            ("CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED", value),
        ])
        .expect("the video switch should never stop a deployment from loading")
        .gemini_candidate_video_enabled
    };

    for on in ["true", "TRUE", "1", "yes", "on"] {
        assert!(video(on), "{on} should send candidate video");
    }
    for unreadable in ["invalid_value", "ture", "2"] {
        assert!(!video(unreadable), "{unreadable} should mean off");
    }
    assert_eq!(video(" "), DEFAULT_GEMINI_CANDIDATE_VIDEO_ENABLED);
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

/// A URL that is present and unusable, which is the other half of the error.
///
/// Blank counts as missing, so every other test in this file reaches
/// `ConfigError` through `missing_keys` alone: nothing drove `invalid_entries`
/// out of `load_from_pairs`, and nothing printed a `ConfigError` carrying both
/// lists. The separator between them had no test at all, so an error naming
/// four faults could have run two sentences together.
#[test]
fn an_unusable_livekit_url_is_named_beside_the_keys_that_are_missing() {
    let Err(error) = load_from_pairs([
        ("LIVEKIT_URL", "ftp://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
    ]) else {
        panic!("a LIVEKIT_URL nothing can dial should fail closed");
    };
    assert_eq!(error.missing_keys, Vec::<&str>::new());
    assert_eq!(
        error.invalid_entries,
        vec!["LIVEKIT_URL must start with wss://, https://, ws:// or http://".to_string()]
    );
    assert_eq!(
        error.to_string(),
        "invalid config entries: LIVEKIT_URL must start with wss://, https://, ws:// or http://"
    );

    // Both faults at once. An operator reading this needs the two sentences to
    // stay two sentences.
    let Err(error) = load_from_pairs([
        ("LIVEKIT_URL", "ftp://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
    ]) else {
        panic!("a bad URL and missing keys should fail closed");
    };
    assert_eq!(
        error.to_string(),
        "missing required config keys: LIVEKIT_API_SECRET, GOOGLE_API_KEY; \
         invalid config entries: LIVEKIT_URL must start with wss://, https://, ws:// or http://"
    );

    // Production is read from the same map, so an unencrypted URL is an invalid
    // entry rather than a missing key.
    let Err(error) = load_from_pairs([
        ("LIVEKIT_URL", "ws://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
        ("NODE_ENV", "production"),
    ]) else {
        panic!("plaintext LiveKit in production should fail closed");
    };
    assert_eq!(
        error.invalid_entries,
        vec!["LIVEKIT_URL must use wss:// or https:// when NODE_ENV=production".to_string()]
    );
}

/// Discovery takes a directory rather than reading the current one, so a test
/// can state exactly what is on disk. That is also the point of the argument:
/// two processes started from two directories used to build two different
/// pools.
/// A provider directory that removes itself when the test ends, panic included.
///
/// Both halves are load bearing. The path carries a pid and a timestamp because
/// the label alone collides between two concurrent `cargo test` runs, and the
/// loser fails with AlreadyExists on a fixture unrelated to what it tests. The
/// Drop is because making the path unique without it just moves the problem:
/// the old `remove_dir_all` at the top only ever matched a path that repeated.
struct ProviderFixture(std::path::PathBuf);

impl Drop for ProviderFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl std::ops::Deref for ProviderFixture {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

fn provider_fixture(label: &str, files: &[(&str, &str)]) -> ProviderFixture {
    let dir = std::env::temp_dir().join(format!(
        "codetrial-providers-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, contents) in files {
        std::fs::write(dir.join(name), contents).unwrap();
    }
    ProviderFixture(dir)
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
            // GitHub-style names, including uppercase letters and dashes, are
            // valid provider ids. Ids are matched exactly, so no name here may
            // gain a case twin: a case-insensitive filesystem would fold the
            // two fixtures into one file and quietly test half of this.
            (
                "codetrial.env.ASIA",
                &provider_file("wss://upper.example", "k", "s"),
            ),
            // The ends of the length range, which is where an off-by-one lives.
            (
                "codetrial.env.a",
                &provider_file("wss://short.example", "k", "s"),
            ),
            (
                &format!("codetrial.env.{}", "b".repeat(39)),
                &provider_file("wss://long.example", "k", "s"),
            ),
            (
                &format!("codetrial.env.{}", "c".repeat(40)),
                &provider_file("wss://toolong.example", "k", "s"),
            ),
            (
                "codetrial.env.eu-west",
                &provider_file("wss://dashed.example", "k", "s"),
            ),
            (
                "codetrial.env.bochengC",
                &provider_file("wss://bocheng.example", "k", "s"),
            ),
            // Taken by the credentials from the environment.
            (
                "codetrial.env.primary",
                &provider_file("wss://clash.example", "k", "s"),
            ),
            // A dash may only sit between two alphanumerics.
            (
                "codetrial.env.bad--name",
                &provider_file("wss://bad.example", "k", "s"),
            ),
            (
                "codetrial.env.-eu",
                &provider_file("wss://lead.example", "k", "s"),
            ),
            (
                "codetrial.env.eu-",
                &provider_file("wss://trail.example", "k", "s"),
            ),
            (
                "codetrial.env.-",
                &provider_file("wss://dash.example", "k", "s"),
            ),
            // Outside the alphabet entirely. Without this the predicate could
            // accept anything and the rest of these fixtures would not notice.
            (
                "codetrial.env.eu_west",
                &provider_file("wss://under.example", "k", "s"),
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
        vec!["ASIA", "a", &"b".repeat(39), "bochengC", "eu", "eu-west"]
    );
    for named in [
        "primary",
        "bad--name",
        "-eu",
        "eu-",
        "eu_west",
        &"c".repeat(40),
        "link",
    ] {
        assert!(
            warnings.iter().any(|warning| warning.contains(named)),
            "no warning names {named}: {warnings:?}"
        );
    }

    // `local` and `example` belong here, so they are not noise. The bare `-` is
    // counted here but not named above: asserting a warning contains "-" would
    // pass on any of them, since the fixture path itself has dashes.
    assert_eq!(warnings.len(), 8, "{warnings:?}");
    assert!(
        !warnings.iter().any(|warning| warning.contains("root:")),
        "a symlink must not be read: {warnings:?}"
    );
}

/// Needs a directory of its own: on a case-insensitive filesystem
/// `codetrial.env.PRIMARY` and `codetrial.env.primary` are one file, so putting
/// both in the fixture above would silently test one of them.
#[test]
fn the_primary_id_is_reserved_whatever_the_casing() {
    for spelling in ["PRIMARY", "Primary", "pRiMaRy"] {
        let dir = provider_fixture(
            &format!("reserved-{spelling}"),
            &[(
                &format!("codetrial.env.{spelling}"),
                &provider_file("wss://clash.example", "k", "s"),
            )],
        );
        let (providers, warnings) = codetrial::config::discover_providers(&dir, false);
        assert!(providers.is_empty(), "{spelling} was loaded: {providers:?}");
        assert!(
            warnings.iter().any(|warning| warning.contains(spelling)),
            "no warning names {spelling}: {warnings:?}"
        );
    }
}

/// Which project leads the rotation is the operator's call, and the ones they
/// did not name still have to stay in it.
#[test]
fn the_operator_chooses_which_provider_leads_the_rotation() {
    use codetrial::config::{Provider, ProviderPool, order_providers};

    let provider = |id: &str| Provider {
        id: id.to_string(),
        url: format!("wss://{id}.example"),
        api_key: "k".to_string(),
        api_secret: "s".to_string(),
        google_api_key: String::new(),
    };
    let pool = || {
        vec![
            provider("primary"),
            provider("bochengC"),
            provider("charliechiou"),
        ]
    };
    let ids = |providers: &[Provider]| {
        providers
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>()
    };

    // Named first, in the order given, and the rest keep the order they had.
    let mut providers = pool();
    assert!(order_providers(&mut providers, "charliechiou").is_empty());
    assert_eq!(ids(&providers), ["charliechiou", "primary", "bochengC"]);

    let mut providers = pool();
    assert!(order_providers(&mut providers, "charliechiou, bochengC").is_empty());
    assert_eq!(ids(&providers), ["charliechiou", "bochengC", "primary"]);

    // No order is the order discovery produced.
    let mut providers = pool();
    assert!(order_providers(&mut providers, "").is_empty());
    assert_eq!(ids(&providers), ["primary", "bochengC", "charliechiou"]);

    // A name nobody answers to is reported rather than dropped in silence, and
    // it must not take a real provider out of the rotation with it.
    let mut providers = pool();
    let warnings = order_providers(&mut providers, "charliechiou,typo,charliechiou");
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings.iter().any(|warning| warning.contains("typo")));
    assert_eq!(ids(&providers), ["charliechiou", "primary", "bochengC"]);

    // The whole reason `primary` stopped meaning "first": a room with no
    // provider segment was minted by the environment credentials, so leading
    // the rotation with another project must not re-point it.
    let mut providers = pool();
    order_providers(&mut providers, "charliechiou");
    let pool = ProviderPool { providers };
    assert_eq!(
        pool.primary().map(|entry| entry.id.as_str()),
        Some("primary")
    );
    assert_eq!(
        pool.for_room("interview-a1b2c3d4", "interview")
            .map(|entry| entry.id.as_str()),
        Some("primary"),
        "a segment-less room still belongs to the environment credentials"
    );
    // And selection really does start where the operator said.
    assert_eq!(pool.select(0).map(|e| e.id.as_str()), Some("charliechiou"));
    assert_eq!(pool.select(1).map(|e| e.id.as_str()), Some("primary"));
    assert_eq!(pool.select(2).map(|e| e.id.as_str()), Some("bochengC"));
}

/// A rotation that visits one project twice is buying less than its length
/// suggests, and `--config <a provider file>` produces exactly that by
/// accident.
#[test]
fn a_pool_says_how_many_projects_it_actually_spreads_over() {
    use codetrial::config::{Provider, pool_summary};

    let at = |id: &str, host: &str| Provider {
        id: id.to_string(),
        url: format!("wss://{host}.livekit.cloud"),
        api_key: "k".to_string(),
        api_secret: "s".to_string(),
        google_api_key: String::new(),
    };

    let (summary, warnings) = pool_summary(&[
        at("primary", "one"),
        at("bochengC", "two"),
        at("charliechiou", "three"),
    ]);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(
        summary.contains("3 provider(s) over 3 project(s)"),
        "{summary}"
    );
    // Rotation order is the point of the line, so it has to be readable in it.
    assert!(
        summary.contains("primary=wss://one.livekit.cloud"),
        "{summary}"
    );

    // The `--config` accident: the environment provider and a discovered file
    // naming one project.
    let (summary, warnings) = pool_summary(&[
        at("primary", "two"),
        at("bochengC", "two"),
        at("charliechiou", "three"),
    ]);
    assert!(
        summary.contains("3 provider(s) over 2 project(s)"),
        "{summary}"
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("primary"), "{warnings:?}");
    assert!(warnings[0].contains("bochengC"), "{warnings:?}");

    // The summary reaches stderr on every start, so it carries the id and the
    // URL the browser is handed anyway, and neither credential.
    let (summary, _) = pool_summary(&[Provider {
        id: "primary".to_string(),
        url: "wss://one.livekit.cloud".to_string(),
        api_key: "APIsentinelkey".to_string(),
        api_secret: "sentinelsecretvalue".to_string(),
        google_api_key: "googlesentinelkey".to_string(),
    }]);
    for secret in ["APIsentinelkey", "sentinelsecretvalue", "googlesentinelkey"] {
        assert!(!summary.contains(secret), "{secret} leaked into: {summary}");
    }
}

/// A value Gemini cannot read is a field Gemini ignores, which silently
/// restores the eager default and shows up only as an interviewer being cut
/// off. So anything unrecognized lands on the safe value, loudly.
#[test]
fn an_unreadable_start_sensitivity_falls_back_rather_than_reaching_the_wire() {
    let load = |value: &str| {
        load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "key"),
            ("LIVEKIT_API_SECRET", "secret"),
            ("GOOGLE_API_KEY", "google"),
            ("GEMINI_START_SENSITIVITY", value),
        ])
        .expect("complete config should load")
        .gemini_start_sensitivity
    };

    // Both spellings of each, because the long ones are shouted API constants.
    for low in [
        "LOW",
        "low",
        "START_SENSITIVITY_LOW",
        "start_sensitivity_low",
    ] {
        assert_eq!(load(low), "START_SENSITIVITY_LOW", "{low}");
    }
    for high in ["HIGH", "high", "START_SENSITIVITY_HIGH"] {
        assert_eq!(load(high), "START_SENSITIVITY_HIGH", "{high}");
    }

    // The typo that would otherwise reach the wire and be dropped there.
    for bad in ["START_SENSITIVITY_LOWW", "medium", "0", "  "] {
        assert_eq!(
            load(bad),
            DEFAULT_GEMINI_START_SENSITIVITY,
            "{bad} must not reach the wire"
        );
    }
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

/// A scheme is not case sensitive, and neither is the check on one.
///
/// This is the bug `livekit_scheme` was extracted to fix, written down: the
/// validator matched `WSS://` and the function that rewrote the URL for a
/// RoomService call did not, so an accepted URL was posted to over a scheme
/// reqwest will not send. Nothing here supplied an upper-case scheme, so both
/// halves of that could regress with the suite green.
#[test]
fn a_livekit_scheme_is_read_the_way_a_url_compares() {
    for url in [
        "WSS://example.livekit.cloud",
        "Https://example.livekit.cloud",
        "WS://127.0.0.1:7880",
    ] {
        assert!(
            codetrial::config::validate_livekit_url(url, false).is_ok(),
            "{url} should validate: a scheme is case insensitive"
        );
    }
    assert!(
        codetrial::config::validate_livekit_url("WS://127.0.0.1:7880", true).is_err(),
        "an upper-case plaintext scheme is still plaintext"
    );

    // Surrounding whitespace is trimmed before any of that, so a value with a
    // stray newline is dialled rather than reported.
    assert!(
        codetrial::config::validate_livekit_url("  wss://example.livekit.cloud\n", false).is_ok()
    );
    assert!(
        codetrial::config::validate_livekit_url("   ", false).is_err(),
        "whitespace is not a URL"
    );
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
    assert_eq!(
        provider_id_from_room("interview-bochengC-a1b2c3d4", "interview"),
        Some("bochengC")
    );
    assert_eq!(
        provider_id_from_room("interview-eu-west-a1b2c3d4", "interview"),
        Some("eu-west")
    );

    // The split is at the LAST dash, not the second. This is the assert that
    // fails if someone reverts `rsplit_once` to `split_once`, which parses this
    // as `a` and routes the agent to a different project than the candidate.
    assert_eq!(
        provider_id_from_room("interview-a-b-c-d1234567", "interview"),
        Some("a-b-c")
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

    // The other half of the feature: the id the name carries has to resolve
    // back to the record that signed it, and a name that carries none has to
    // land on the project a single-provider deployment mints against. Both
    // sides call `for_room` rather than spelling the rule out twice.
    let pool = codetrial::config::ProviderPool {
        providers: vec![
            provider(PRIMARY_PROVIDER_ID, "wss://home.livekit.cloud"),
            provider("eu", "wss://eu.livekit.cloud"),
        ],
    };
    assert_eq!(
        pool.for_room("interview-eu-a1b2c3d4", "interview")
            .map(|found| found.id.as_str()),
        Some("eu")
    );
    assert_eq!(
        pool.for_room("interview-a1b2c3d4", "interview")
            .map(|found| found.id.as_str()),
        Some(PRIMARY_PROVIDER_ID),
        "a name with no segment belongs to the project that mints unsegmented names"
    );
    assert_eq!(
        pool.for_room("interview-gone-a1b2c3d4", "interview")
            .map(|found| found.id.as_str()),
        Some(PRIMARY_PROVIDER_ID),
        "a retired id falls back rather than failing the room"
    );

    // A pool built entirely from files has no `primary` entry, and some project
    // still has to answer.
    let files_only = codetrial::config::ProviderPool {
        providers: vec![
            provider("eu", "wss://eu.livekit.cloud"),
            provider("us", "wss://us.livekit.cloud"),
        ],
    };
    assert_eq!(
        files_only.primary().map(|found| found.id.as_str()),
        Some("eu"),
        "with nothing named primary the front of the pool leads"
    );
}

/// A pool entry with only the two fields the lookup rules read.
fn provider(id: &str, url: &str) -> codetrial::config::Provider {
    codetrial::config::Provider {
        id: id.to_string(),
        url: url.to_string(),
        api_key: "key".to_string(),
        api_secret: "secret".to_string(),
        google_api_key: "google".to_string(),
    }
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
            "LIVEKIT_URL=wss://frankfurt.example\nLIVEKIT_API_KEY=livekit-key\nLIVEKIT_API_SECRET=hunter2\nGOOGLE_API_KEY=gsecret\n",
        )],
    );
    let (providers, _) = codetrial::config::discover_providers(&dir, false);

    let rendered = format!("{:?}", providers[0]);
    assert!(!rendered.contains("livekit-key"), "{rendered}");
    assert!(!rendered.contains("hunter2"), "{rendered}");
    assert!(!rendered.contains("gsecret"), "{rendered}");

    // The id and URL are what makes the line useful, and neither is a
    // credential. Named against a host that is not the id: this used to look
    // for "eu" in a line that carried `wss://eu.example`, so it held whether or
    // not `Debug` printed the id at all.
    assert!(rendered.contains("eu"), "{rendered}");
    assert!(rendered.contains("frankfurt.example"), "{rendered}");
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

/// The recording block, which is off in every deployment until an operator
/// provisions the resources in `docs/recording-contract.md`. These read the
/// parser directly rather than through `load_from_pairs`, because
/// `codetrial web` runs without a Gemini key and still has to validate this.
mod recording {
    use std::collections::BTreeMap;

    use codetrial::config::{
        DEFAULT_RECORDING_BITRATE, DEFAULT_RECORDING_GCS_PREFIX, DEFAULT_RECORDING_MAX_MINUTES,
        PRIMARY_PROVIDER_ID, Provider, ProviderPool, load_recording,
    };

    const SERVICE_ACCOUNT: &str = r#"{"type":"service_account","client_email":"codetrial@example.iam.gserviceaccount.com","private_key":"KEYMATERIAL-4bd2"}"#;
    const RECORDING_SECRET: &str = "recording-api-secret-9f3c";
    const RECORDING_KEY: &str = "APIrecordingkey";

    fn pool() -> ProviderPool {
        ProviderPool {
            providers: vec![Provider {
                id: PRIMARY_PROVIDER_ID.to_string(),
                url: "wss://project.livekit.cloud".to_string(),
                api_key: "devkey".to_string(),
                api_secret: "devsecret".to_string(),
                google_api_key: "google".to_string(),
            }],
        }
    }

    fn values(
        pairs: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) -> BTreeMap<String, String> {
        let mut values: BTreeMap<String, String> = [
            ("CODETRIAL_RECORDING_ENABLED", "true"),
            ("CODETRIAL_RECORDING_GCS_BUCKET", "codetrial-staging"),
            ("CODETRIAL_RECORDING_DRIVE_ID", "0AKfixtureDriveId"),
            ("CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON", SERVICE_ACCOUNT),
            (
                "CODETRIAL_RECORDING_TEMPLATE_BASE_URL",
                "https://recording.codetrial.example",
            ),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
        for (key, value) in pairs {
            if value.is_empty() {
                values.remove(key);
            } else {
                values.insert(key.to_string(), value.to_string());
            }
        }
        values
    }

    #[test]
    fn recording_is_off_unless_it_is_turned_on() {
        assert_eq!(
            load_recording(&BTreeMap::new(), &pool(), 45, false),
            Ok(None),
            "the default deployment records nothing and needs no credentials to say so"
        );
    }

    #[test]
    fn recording_accepts_complete_configuration() {
        let config = load_recording(&values([]), &pool(), 45, false)
            .expect("a complete recording block should load")
            .expect("enabled recording should produce a config");

        assert_eq!(config.gcs_bucket, "codetrial-staging");
        assert_eq!(config.drive_id, "0AKfixtureDriveId");
        assert_eq!(config.gcs_prefix, DEFAULT_RECORDING_GCS_PREFIX);
        assert_eq!(config.max_minutes, DEFAULT_RECORDING_MAX_MINUTES);
        assert_eq!(config.bitrate, DEFAULT_RECORDING_BITRATE);
        assert!(!config.kill_switch);
        assert!(
            config.livekit.is_none(),
            "with no override, recording follows the project that owns the room"
        );

        // The trailing slash goes here rather than at every call site that
        // appends the template path.
        let trailing = load_recording(
            &values([(
                "CODETRIAL_RECORDING_TEMPLATE_BASE_URL",
                "https://recording.codetrial.example/",
            )]),
            &pool(),
            45,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            trailing.template_base_url,
            "https://recording.codetrial.example"
        );
    }

    #[test]
    fn recording_accepts_a_livekit_override_naming_a_pooled_project() {
        let config = load_recording(
            &values([
                (
                    "CODETRIAL_RECORDING_LIVEKIT_URL",
                    "wss://project.livekit.cloud",
                ),
                ("CODETRIAL_RECORDING_LIVEKIT_API_KEY", RECORDING_KEY),
                ("CODETRIAL_RECORDING_LIVEKIT_API_SECRET", RECORDING_SECRET),
            ]),
            &pool(),
            45,
            false,
        )
        .expect("an override naming a pooled project should load")
        .unwrap();

        let livekit = config.livekit.expect("the override should be recorded");
        assert_eq!(livekit.api_key, RECORDING_KEY);
        assert_eq!(livekit.api_secret, RECORDING_SECRET);
    }

    #[test]
    fn recording_rejects_partial_credentials() {
        for missing in [
            "CODETRIAL_RECORDING_GCS_BUCKET",
            "CODETRIAL_RECORDING_DRIVE_ID",
            "CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON",
            "CODETRIAL_RECORDING_TEMPLATE_BASE_URL",
        ] {
            let error = load_recording(&values([(missing, "")]), &pool(), 45, false)
                .expect_err("a missing credential must fail closed");
            assert!(
                error.missing_keys.contains(&missing),
                "{missing} should be named as missing, got {:?}",
                error.missing_keys
            );
        }

        // The LiveKit override is all three or none. Two of three is a project
        // nothing can call, and defaulting the third would silently record
        // somewhere the operator did not name.
        let error = load_recording(
            &values([(
                "CODETRIAL_RECORDING_LIVEKIT_URL",
                "wss://project.livekit.cloud",
            )]),
            &pool(),
            45,
            false,
        )
        .expect_err("a half-configured override must fail closed");
        assert!(
            error
                .missing_keys
                .contains(&"CODETRIAL_RECORDING_LIVEKIT_API_KEY")
        );
        assert!(
            error
                .missing_keys
                .contains(&"CODETRIAL_RECORDING_LIVEKIT_API_SECRET")
        );
    }

    #[test]
    fn recording_rejects_max_minutes_below_interview_duration() {
        let error = load_recording(
            &values([("CODETRIAL_RECORDING_MAX_MINUTES", "30")]),
            &pool(),
            45,
            false,
        )
        .expect_err("a recording that ends before the interview is worthless");
        assert!(
            error
                .invalid_entries
                .iter()
                .any(|entry| entry.contains("CODETRIAL_RECORDING_MAX_MINUTES")),
            "the reason should name the key, got {:?}",
            error.invalid_entries
        );

        // Equal is fine: the recording covers exactly the interview. Asserted
        // as `Some`, not as `is_ok`, because `Ok(None)` is also `is_ok` and
        // means recording never loaded at all.
        assert!(
            load_recording(
                &values([("CODETRIAL_RECORDING_MAX_MINUTES", "45")]),
                &pool(),
                45,
                false
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn recording_rejects_a_value_it_cannot_read() {
        // `optional_u32` defaults on a parse failure, which is right for a
        // tuning knob and wrong for a bill: a server that starts and records at
        // whatever the default happened to be is worse than one that refuses.
        for (key, value) in [
            ("CODETRIAL_RECORDING_BITRATE", "2 Mbps"),
            ("CODETRIAL_RECORDING_MAX_MINUTES", "forty-five"),
        ] {
            let error = load_recording(&values([(key, value)]), &pool(), 45, false)
                .unwrap_err()
                .to_string();
            assert!(error.contains(key), "{key} should be named, got {error}");
        }

        // And a mistyped switch is a startup failure, not a silent opt-out.
        // Failing closed by accident still means no consent prompt, no
        // artifact, and nothing said.
        for key in [
            "CODETRIAL_RECORDING_ENABLED",
            "CODETRIAL_RECORDING_KILL_SWITCH",
        ] {
            let error = load_recording(&values([(key, "ture")]), &pool(), 45, false)
                .unwrap_err()
                .to_string();
            assert!(error.contains(key), "{key} should be named, got {error}");
        }
    }

    #[test]
    fn recording_stays_off_without_grading_settings_nothing_reads() {
        // A deployment that does not record must not fail to start over a typo
        // in a value that is ignored while the switch is off.
        let mut off: BTreeMap<String, String> = BTreeMap::new();
        off.insert(
            "CODETRIAL_RECORDING_ENABLED".to_string(),
            "false".to_string(),
        );
        off.insert(
            "CODETRIAL_RECORDING_KILL_SWITCH".to_string(),
            "ture".to_string(),
        );
        off.insert(
            "CODETRIAL_RECORDING_BITRATE".to_string(),
            "2 Mbps".to_string(),
        );
        assert_eq!(load_recording(&off, &pool(), 45, false), Ok(None));
    }

    #[test]
    fn recording_tells_two_ports_on_one_host_apart() {
        // A bracketed IPv6 authority hides its port behind colons that belong
        // to the address. Reading the last colon as the separator refuses the
        // port and treats `[addr]:8443` as the default, which would accept an
        // override for a different endpoint.
        let ipv6_pool = ProviderPool {
            providers: vec![Provider {
                id: PRIMARY_PROVIDER_ID.to_string(),
                url: "wss://[2001:db8::1]".to_string(),
                api_key: "devkey".to_string(),
                api_secret: "devsecret".to_string(),
                google_api_key: "google".to_string(),
            }],
        };
        let override_at = |url: &'static str| {
            load_recording(
                &values([
                    ("CODETRIAL_RECORDING_LIVEKIT_URL", url),
                    ("CODETRIAL_RECORDING_LIVEKIT_API_KEY", RECORDING_KEY),
                    ("CODETRIAL_RECORDING_LIVEKIT_API_SECRET", RECORDING_SECRET),
                ]),
                &ipv6_pool,
                45,
                false,
            )
        };
        assert!(
            override_at("wss://[2001:db8::1]").is_ok(),
            "the same endpoint must still match"
        );
        assert!(
            override_at("wss://[2001:db8::1]:8443").is_err(),
            "a different port is a different project"
        );
    }

    #[test]
    fn recording_matches_a_pooled_project_across_equivalent_urls() {
        // `wss://host` and `https://host:443` are one endpoint. Reporting them
        // as different projects would refuse a correct configuration.
        //
        // Asserted through `livekit`, not through `is_ok`: `Ok(None)` is also
        // `is_ok` and means recording never loaded, which is how this could
        // have gone green against a build that stopped reading the override.
        let matched = |pool: &ProviderPool, url: &'static str| {
            load_recording(
                &values([
                    ("CODETRIAL_RECORDING_LIVEKIT_URL", url),
                    ("CODETRIAL_RECORDING_LIVEKIT_API_KEY", RECORDING_KEY),
                    ("CODETRIAL_RECORDING_LIVEKIT_API_SECRET", RECORDING_SECRET),
                ]),
                pool,
                45,
                false,
            )
            .map(|config| config.and_then(|config| config.livekit).is_some())
        };
        for equivalent in [
            "https://project.livekit.cloud",
            "wss://project.livekit.cloud:443",
            "wss://PROJECT.livekit.cloud",
        ] {
            assert_eq!(
                matched(&pool(), equivalent),
                Ok(true),
                "{equivalent} names the pooled project"
            );
        }

        // The plaintext pair is the same rule at the other default port, and it
        // is the pair a developer runs against: `ws://host` and
        // `http://host:80` are one endpoint, while `wss://host` is a second one
        // on port 443. Nothing reached the `ws`/`http` arm, so it could have
        // defaulted to 443 with every test still green.
        let local = ProviderPool {
            providers: vec![Provider {
                id: PRIMARY_PROVIDER_ID.to_string(),
                url: "ws://localhost".to_string(),
                api_key: "devkey".to_string(),
                api_secret: "devsecret".to_string(),
                google_api_key: "google".to_string(),
            }],
        };
        assert_eq!(matched(&local, "http://localhost:80"), Ok(true));
        assert_eq!(matched(&local, "ws://localhost:80"), Ok(true));
        assert!(
            matched(&local, "wss://localhost").is_err(),
            "443 is not 80, so this is a different endpoint"
        );
    }

    /// A LiveKit override the pool cannot match because it is not a URL.
    ///
    /// `recording_livekit` reports an unusable URL and a URL naming an unpooled
    /// project through two different branches with two different messages, and
    /// only the second had a test: a build that stopped validating the override
    /// would have fallen into the pool check and refused for the wrong reason,
    /// or accepted `wss://` with no host as a project it could dial.
    #[test]
    fn recording_rejects_an_override_url_it_cannot_read() {
        for (url, expected) in [
            ("ftp://project.livekit.cloud", "must start with wss://"),
            ("wss://", "scheme but no host"),
            ("project.livekit.cloud", "must start with wss://"),
        ] {
            let error = load_recording(
                &values([
                    ("CODETRIAL_RECORDING_LIVEKIT_URL", url),
                    ("CODETRIAL_RECORDING_LIVEKIT_API_KEY", RECORDING_KEY),
                    ("CODETRIAL_RECORDING_LIVEKIT_API_SECRET", RECORDING_SECRET),
                ]),
                &pool(),
                45,
                false,
            )
            .expect_err("an override nothing can dial must fail closed");
            assert!(
                error.invalid_entries.iter().any(|entry| entry
                    .starts_with("CODETRIAL_RECORDING_LIVEKIT_URL:")
                    && entry.contains(expected)),
                "{url} should be refused as unreadable, got {:?}",
                error.invalid_entries
            );
        }
    }

    /// The bitrate ceiling and floor, which nothing asserted.
    ///
    /// `recording_number` already refuses a value it cannot parse; this is the
    /// separate check that a number it can parse is one Egress will encode at.
    /// A deployment that recorded at 40 kbps would produce a file, bill for it,
    /// and hand a candidate something unwatchable.
    #[test]
    fn recording_rejects_a_bitrate_outside_the_range_egress_can_encode() {
        use codetrial::config::{MAX_RECORDING_BITRATE, MIN_RECORDING_BITRATE};

        for refused in ["0", "100", "9000"] {
            let error = load_recording(
                &values([("CODETRIAL_RECORDING_BITRATE", refused)]),
                &pool(),
                45,
                false,
            )
            .expect_err("a bitrate outside the range must fail closed");
            assert!(
                error
                    .invalid_entries
                    .iter()
                    .any(|entry| entry.contains("CODETRIAL_RECORDING_BITRATE")),
                "{refused} kbps should be named, got {:?}",
                error.invalid_entries
            );
        }

        // Both ends are inclusive, so the bounds themselves load.
        for accepted in [MIN_RECORDING_BITRATE, MAX_RECORDING_BITRATE] {
            let mut pairs = values([]);
            pairs.insert(
                "CODETRIAL_RECORDING_BITRATE".to_string(),
                accepted.to_string(),
            );
            assert_eq!(
                load_recording(&pairs, &pool(), 45, false)
                    .unwrap()
                    .map(|config| config.bitrate),
                Some(accepted),
                "{accepted} is inside the range"
            );
        }
    }

    /// The credential is read at startup, not at the first delivery.
    ///
    /// `codetrial::delivery::readable_service_account` had no test anywhere:
    /// replacing its body with `Ok(())` left the whole suite green, and the
    /// deployment it describes records interviews it can never hand over and
    /// finds out an hour into the first one.
    #[test]
    fn recording_refuses_a_service_account_it_cannot_read() {
        for (broken, expected) in [
            ("not json at all", "not JSON"),
            (r#"{"private_key":"KEYMATERIAL-4bd2"}"#, "no client_email"),
            (
                r#"{"client_email":"","private_key":"KEYMATERIAL-4bd2"}"#,
                "no client_email",
            ),
            (
                r#"{"client_email":"codetrial@example.iam.gserviceaccount.com"}"#,
                "no private_key",
            ),
            (
                r#"{"client_email":"codetrial@example.iam.gserviceaccount.com","private_key":""}"#,
                "no private_key",
            ),
        ] {
            let mut pairs = values([]);
            pairs.insert(
                "CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON".to_string(),
                broken.to_string(),
            );
            let error = load_recording(&pairs, &pool(), 45, false)
                .expect_err("a credential nobody can sign with must fail closed");
            assert!(
                error.invalid_entries.iter().any(|entry| entry
                    .starts_with("CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON:")
                    && entry.contains(expected)),
                "{expected} should be reported, got {:?}",
                error.invalid_entries
            );

            // The reason reaches stderr, so it must not carry the credential.
            for entry in &error.invalid_entries {
                assert!(
                    !entry.contains("KEYMATERIAL-4bd2"),
                    "the key material must not appear in {entry}"
                );
            }
        }

        assert_eq!(
            codetrial::delivery::readable_service_account(SERVICE_ACCOUNT),
            Ok(())
        );
    }

    /// The two settings that were only ever asserted at their defaults.
    ///
    /// Nothing turned the kill switch on or moved the staging prefix, so a
    /// parser that ignored either key and answered from the default was
    /// indistinguishable from one that read it. The kill switch is the control
    /// an operator reaches for when recording has to stop now.
    #[test]
    fn recording_reads_the_kill_switch_and_the_prefix_rather_than_defaulting_them() {
        let config = |key: &'static str, value: &'static str| {
            load_recording(&values([(key, value)]), &pool(), 45, false)
                .expect("the switch and the prefix should load")
                .expect("recording is enabled")
        };

        for on in ["true", "TRUE", "1", "yes", "on"] {
            assert!(
                config("CODETRIAL_RECORDING_KILL_SWITCH", on).kill_switch,
                "{on} should stop recording"
            );
        }
        for off in ["false", "0", "no", "off"] {
            assert!(
                !config("CODETRIAL_RECORDING_KILL_SWITCH", off).kill_switch,
                "{off} should leave recording running"
            );
        }

        assert_eq!(
            config("CODETRIAL_RECORDING_GCS_PREFIX", "staging/runs").gcs_prefix,
            "staging/runs"
        );
    }

    #[test]
    fn recording_rejects_unknown_provider_url() {
        let error = load_recording(
            &values([
                (
                    "CODETRIAL_RECORDING_LIVEKIT_URL",
                    "wss://elsewhere.livekit.cloud",
                ),
                ("CODETRIAL_RECORDING_LIVEKIT_API_KEY", RECORDING_KEY),
                ("CODETRIAL_RECORDING_LIVEKIT_API_SECRET", RECORDING_SECRET),
            ]),
            &pool(),
            45,
            false,
        )
        .expect_err("recording a project that holds no rooms records nothing");
        assert!(
            error
                .invalid_entries
                .iter()
                .any(|entry| entry.contains("provider pool")),
            "got {:?}",
            error.invalid_entries
        );
    }

    #[test]
    fn recording_accepts_a_public_host_that_looks_like_a_private_prefix() {
        // The prefix-matching version of this check refused every hostname
        // beginning `fc` or `fd`, which includes `facebook.com`, and every one
        // beginning `10.` or `127.`.
        for public in [
            "https://fc-cdn.example.com",
            "https://fd-recording.example",
            "https://10.example.com",
            "https://127.example.com",
            "https://recording.localhostess.example",
            "https://recording.codetrial.example:8443",
        ] {
            // Asserted through the value that comes back, not through `is_ok`:
            // `Ok(None)` is also `is_ok` and means recording never loaded, so a
            // break anywhere before the URL check would have passed for
            // acceptance.
            assert_eq!(
                load_recording(
                    &values([("CODETRIAL_RECORDING_TEMPLATE_BASE_URL", public)]),
                    &pool(),
                    45,
                    false,
                )
                .map(|config| config.map(|config| config.template_base_url)),
                Ok(Some(public.to_string())),
                "{public} is a public name and must be accepted"
            );
        }
    }

    #[test]
    fn recording_rejects_a_template_url_egress_cannot_fetch() {
        for unreachable in [
            "http://recording.codetrial.example",
            "https://127.0.0.1:8443",
            "https://127.1.2.3",
            "https://localhost",
            "https://localhost.",
            "https://api.localhost",
            "https://[::1]:8443",
            "https://[::ffff:127.0.0.1]",
            "https://[fd00::1]",
            "https://[FE80::1]",
            "https://10.1.2.3",
            "https://172.16.0.1",
            "https://192.168.1.1",
            "https://169.254.169.254",
            "https://0.0.0.0",
            // A credential hiding inside a URL survives every redaction that
            // was written to catch a credential in a field of its own.
            "https://user:hunter2@recording.codetrial.example",
            "https://recording.codetrial.example/?layout=grid",
            // A port Egress cannot connect to is as unusable as a host it
            // cannot reach.
            "https://recording.codetrial.example:0",
            "https://recording.codetrial.example:not-a-port",
            "https://recording.codetrial.example:99999",
            // A truncated bracketed authority is not a host.
            "https://[2001:db8::1",
            "https://[2001:db8::1]junk",
        ] {
            let error = load_recording(
                &values([("CODETRIAL_RECORDING_TEMPLATE_BASE_URL", unreachable)]),
                &pool(),
                45,
                false,
            )
            .unwrap_err();
            assert!(
                error
                    .invalid_entries
                    .iter()
                    .any(|entry| entry.contains("CODETRIAL_RECORDING_TEMPLATE_BASE_URL")),
                "{unreachable} should be refused, got {error:?}"
            );
        }
    }

    #[test]
    fn recording_redacts_secrets() {
        let config = load_recording(
            &values([
                (
                    "CODETRIAL_RECORDING_LIVEKIT_URL",
                    "wss://project.livekit.cloud",
                ),
                ("CODETRIAL_RECORDING_LIVEKIT_API_KEY", RECORDING_KEY),
                ("CODETRIAL_RECORDING_LIVEKIT_API_SECRET", RECORDING_SECRET),
            ]),
            &pool(),
            45,
            false,
        )
        .unwrap()
        .unwrap();

        // `WebServerConfig` derives `Debug`, so anything reachable from it can
        // reach a log line. The private key is the worst of these: it is key
        // material living in a `String`.
        let printed = format!("{config:?}");
        for secret in [
            SERVICE_ACCOUNT,
            RECORDING_SECRET,
            RECORDING_KEY,
            "KEYMATERIAL-4bd2",
        ] {
            assert!(
                !printed.contains(secret),
                "Debug output leaked {secret}: {printed}"
            );
        }
        assert!(
            printed.contains("<redacted>"),
            "the redaction should be visible rather than silent: {printed}"
        );

        // Validation messages reach stderr too, so every rejection has to be
        // reason-only. This one fails on three counts at once.
        let error = load_recording(
            &values([
                (
                    "CODETRIAL_RECORDING_LIVEKIT_URL",
                    "wss://elsewhere.livekit.cloud",
                ),
                ("CODETRIAL_RECORDING_LIVEKIT_API_KEY", RECORDING_KEY),
                ("CODETRIAL_RECORDING_LIVEKIT_API_SECRET", RECORDING_SECRET),
                ("CODETRIAL_RECORDING_MAX_MINUTES", "5"),
                ("CODETRIAL_RECORDING_BITRATE", "999999"),
            ]),
            &pool(),
            45,
            false,
        )
        .unwrap_err();
        let message = error.to_string();
        for secret in [
            SERVICE_ACCOUNT,
            RECORDING_SECRET,
            RECORDING_KEY,
            "KEYMATERIAL-4bd2",
        ] {
            assert!(
                !message.contains(secret),
                "a validation message leaked {secret}: {message}"
            );
        }
        assert!(
            !message.contains("elsewhere.livekit.cloud"),
            "a LiveKit URL can carry a query string, so it never appears in a message: {message}"
        );

        // The same argument applies to `Debug`, which is why the override
        // prints its host rather than its URL.
        let with_credentials = load_recording(
            &values([
                (
                    "CODETRIAL_RECORDING_LIVEKIT_URL",
                    "wss://apikey:hunter2@project.livekit.cloud/?token=leaked",
                ),
                ("CODETRIAL_RECORDING_LIVEKIT_API_KEY", RECORDING_KEY),
                ("CODETRIAL_RECORDING_LIVEKIT_API_SECRET", RECORDING_SECRET),
            ]),
            &pool(),
            45,
            false,
        )
        .unwrap()
        .unwrap();
        let printed = format!("{with_credentials:?}");
        for secret in ["hunter2", "leaked", "apikey"] {
            assert!(
                !printed.contains(secret),
                "Debug leaked {secret} out of a URL: {printed}"
            );
        }
    }
}

/// A cap the operator set and the server cannot honor is said out loud.
///
/// Falling back silently gives a node that was meant to be draining sixteen
/// fresh interviews, with nothing in the log to explain why the number in the
/// config file had no effect.
#[test]
fn an_unusable_interview_cap_falls_back_and_says_so() {
    let cap = |value: &str| {
        let mut warnings = Vec::new();
        let values: BTreeMap<String, String> = std::iter::once((
            "CODETRIAL_MAX_CONCURRENT_INTERVIEWS".to_string(),
            value.to_string(),
        ))
        .collect();
        let resolved = max_concurrent_interviews(&values, &mut warnings);
        (resolved, warnings)
    };
    let default = DEFAULT_MAX_CONCURRENT_INTERVIEWS;

    assert_eq!(
        cap("4"),
        (4, Vec::new()),
        "a usable cap is taken and not remarked on"
    );

    let (zero, warnings) = cap("0");
    assert_eq!(zero, default);
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("refuse every interview")),
        "zero must say what it would have done: {warnings:?}"
    );

    let (typo, warnings) = cap("sixteeen");
    assert_eq!(typo, default);
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("not a positive whole number")),
        "an unreadable cap must name itself: {warnings:?}"
    );

    // Unset is the default, and the default is not worth a word.
    let mut warnings = Vec::new();
    assert_eq!(
        max_concurrent_interviews(&BTreeMap::new(), &mut warnings),
        default
    );
    assert!(
        warnings.is_empty(),
        "an unset cap is not a warning: {warnings:?}"
    );
}

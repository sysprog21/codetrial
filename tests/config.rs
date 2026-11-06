#![expect(
    clippy::unwrap_used,
    reason = "a test that fails by panicking is working as intended"
)]

use codetrial::config::{
    DEFAULT_COMPILER_EXPLORER_ENABLED, DEFAULT_DURATION_MIN,
    DEFAULT_GEMINI_CANDIDATE_VIDEO_ENABLED, DEFAULT_GEMINI_END_SENSITIVITY,
    DEFAULT_GEMINI_FIRST_OUTPUT_S, DEFAULT_GEMINI_LIVE_MODEL, DEFAULT_GEMINI_REPLY_TIMEOUT_S,
    DEFAULT_GEMINI_REPORT_MODEL, DEFAULT_GEMINI_SILENCE_MS, DEFAULT_GEMINI_START_SENSITIVITY,
    DEFAULT_GEMINI_VOICE, DEFAULT_MAX_CONCURRENT_INTERVIEWS, DEFAULT_MAX_INTERIM_REVIEWS,
    DEFAULT_ROOM_PREFIX, MAX_GEMINI_REPLY_TIMEOUT_S, MAX_GEMINI_SILENCE_MS, MAX_INTERIM_REVIEWS,
    MIN_GEMINI_FIRST_OUTPUT_S, MIN_GEMINI_REPLY_TIMEOUT_S, guest_mode_enabled, load_from_pairs,
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
    assert_eq!(config.google_api_keys, ["google"]);
    assert_eq!(config.gemini_live_model, "live-model");
    assert_eq!(config.gemini_report_model, "report-model");
    assert_eq!(config.gemini_voice, "Voice");
    assert_eq!(config.gemini_silence_ms, DEFAULT_GEMINI_SILENCE_MS);
    assert_eq!(config.room_prefix, "room");
    assert_eq!(config.default_duration_min, 30);
    assert!(config.gemini_candidate_video_enabled);
}

/// The reply watchdog is tunable from the latency an operator sees in the
/// `timing:` lines, but never below the prompt stall it would race, nor so
/// high that a silent provider is left unrecovered.
#[test]
fn config_bounds_the_reply_timeout() {
    let base = [
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
    ];
    let timeout = |value: Option<&'static str>| {
        load_from_pairs(
            base.into_iter()
                .chain(value.map(|value| ("CODETRIAL_GEMINI_REPLY_TIMEOUT_S", value))),
        )
        .expect("config")
        .gemini_reply_timeout_s
    };
    assert_eq!(timeout(None), DEFAULT_GEMINI_REPLY_TIMEOUT_S);
    assert_eq!(timeout(Some("60")), 60);
    assert_eq!(timeout(Some("5")), MIN_GEMINI_REPLY_TIMEOUT_S);
    assert_eq!(timeout(Some("9999")), MAX_GEMINI_REPLY_TIMEOUT_S);
    assert_eq!(timeout(Some("soon")), DEFAULT_GEMINI_REPLY_TIMEOUT_S);
}

/// build.rs reads the same `GITHUB_SHA` this reads, so in CI, where both see
/// it, a build.rs that stops recording the release commit fails here rather
/// than leaving every release's update notice silent with nothing else red.
/// A local build sees neither and records nothing.
#[test]
fn a_ci_build_records_its_commit_as_the_release_commit() {
    let expected = std::env::var("GITHUB_SHA")
        .ok()
        .map(|sha| sha.trim().to_ascii_lowercase())
        .filter(|sha| sha.len() >= 7 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(|sha| sha[..7].to_string())
        .unwrap_or_default();
    assert_eq!(env!("CODETRIAL_RELEASE_COMMIT"), expected);
}

#[test]
fn the_tool_acknowledgement_is_off_unless_asked_for() {
    let base = [
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
    ];
    let ack = |value: Option<&'static str>| {
        load_from_pairs(
            base.into_iter()
                .chain(value.map(|value| ("CODETRIAL_GEMINI_TOOL_ACK", value))),
        )
        .expect("config")
        .gemini_tool_ack
    };
    assert!(!ack(None));
    assert!(ack(Some("1")));
    assert!(!ack(Some("0")));
}

#[test]
fn config_bounds_the_first_output_deadline() {
    let base = [
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
    ];
    let deadline = |value: Option<&'static str>| {
        load_from_pairs(
            base.into_iter()
                .chain(value.map(|value| ("CODETRIAL_GEMINI_FIRST_OUTPUT_S", value))),
        )
        .expect("config")
        .gemini_first_output_s
    };
    assert_eq!(deadline(None), DEFAULT_GEMINI_FIRST_OUTPUT_S);
    assert_eq!(deadline(Some("30")), 30);
    assert_eq!(deadline(Some("1")), MIN_GEMINI_FIRST_OUTPUT_S);
    assert_eq!(deadline(Some("9999")), DEFAULT_GEMINI_REPLY_TIMEOUT_S);
    assert_eq!(deadline(Some("soon")), DEFAULT_GEMINI_FIRST_OUTPUT_S);

    // Never above the reply timeout actually configured, not only the largest
    // one allowed.
    let capped = load_from_pairs(base.into_iter().chain([
        ("CODETRIAL_GEMINI_REPLY_TIMEOUT_S", "25"),
        ("CODETRIAL_GEMINI_FIRST_OUTPUT_S", "60"),
    ]))
    .expect("config");
    assert_eq!(capped.gemini_first_output_s, 25);
}

#[test]
fn config_bounds_interim_review_spending() {
    let base = [
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
    ];
    assert_eq!(
        load_from_pairs(base)
            .expect("default config")
            .max_interim_reviews,
        DEFAULT_MAX_INTERIM_REVIEWS,
    );
    assert_eq!(
        load_from_pairs(
            base.into_iter()
                .chain([("CODETRIAL_MAX_INTERIM_REVIEWS", "999")]),
        )
        .expect("bounded config")
        .max_interim_reviews,
        MAX_INTERIM_REVIEWS,
    );
    assert_eq!(
        load_from_pairs(
            base.into_iter()
                .chain([("CODETRIAL_MAX_INTERIM_REVIEWS", "0")]),
        )
        .expect("disabled config")
        .max_interim_reviews,
        0,
    );
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

    // Literals for the same reason. Together these decide how long every reply
    // to unyielded speech waits, and moving either is a product decision.
    assert_eq!(config.gemini_silence_ms, 1_500);
    assert_eq!(config.gemini_end_sensitivity, "END_SENSITIVITY_HIGH");
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

#[test]
fn guest_mode_is_disabled_by_default_and_fails_closed() {
    assert!(!guest_mode_enabled(None));
    assert!(!guest_mode_enabled(Some("")));
    assert!(!guest_mode_enabled(Some("   ")));

    for on in ["true", "TRUE", "1", "yes", "on"] {
        assert!(guest_mode_enabled(Some(on)), "{on} should mean on");
    }

    for off in ["false", "FALSE", "0", "no", "off"] {
        assert!(!guest_mode_enabled(Some(off)), "{off} should mean off");
    }

    for invalid in ["flase", "maybe", "2"] {
        assert!(
            !guest_mode_enabled(Some(invalid)),
            "{invalid} should mean off"
        );
    }
}

/// Off unless asked for: a server whose stdin is `/dev/null`, which is every
/// service manager's default, would otherwise exit the moment it started.
#[test]
fn exiting_with_stdin_is_only_on_when_a_launcher_asks() {
    let exits = codetrial::config::exit_with_stdin;
    for off in [None, Some(""), Some("0"), Some("false"), Some("maybe")] {
        assert!(!exits(off), "{off:?} should leave it off");
    }
    for on in ["1", "true", "yes"] {
        assert!(exits(Some(on)), "{on} should turn it on");
    }
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

    // A directory cannot be a provider file and must be reported.
    std::fs::create_dir(dir.join("codetrial.env.directory")).unwrap();

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
        "directory",
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
}

/// A symlink is never followed, so it has to be reported rather than dropped.
#[cfg(unix)]
#[test]
fn a_provider_symlink_is_not_followed() {
    let dir = provider_fixture("provider-symlink", &[]);
    let target = dir.join("target");
    std::fs::write(
        &target,
        provider_file("wss://linked.example", "key", "secret"),
    )
    .unwrap();
    std::os::unix::fs::symlink(&target, dir.join("codetrial.env.link")).unwrap();
    let (providers, warnings) = codetrial::config::discover_providers(&dir, false);
    assert!(providers.is_empty(), "a symlink must not be read");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("link"));
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
        google_api_keys: Vec::new(),
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

    // The two ways to miss are told apart: a mistyped name is not blamed on a
    // duplicate, and a repeated one is not called unknown.
    assert!(
        warnings[0].contains("no provider is named typo"),
        "{warnings:?}"
    );
    assert!(
        warnings[1].contains("charliechiou is listed more than once"),
        "{warnings:?}"
    );
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
        google_api_keys: Vec::new(),
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
    assert!(summary.contains("primary=one.livekit.cloud"), "{summary}");

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
    // endpoint authority, never userinfo, paths, queries or fragments.
    let (summary, _) = pool_summary(&[Provider {
        id: "primary".to_string(),
        url: "wss://urluser:urlpassword@one.livekit.cloud:9443/privatepath?token=querycredential#fragmentcredential".to_string(),
        api_key: "APIsentinelkey".to_string(),
        api_secret: "sentinelsecretvalue".to_string(),
        google_api_keys: vec!["googlesentinelkey".to_string()],
    }]);
    assert!(
        summary.contains("primary=one.livekit.cloud:9443"),
        "{summary}"
    );
    for secret in [
        "APIsentinelkey",
        "sentinelsecretvalue",
        "googlesentinelkey",
        "urluser",
        "urlpassword",
        "privatepath",
        "querycredential",
        "fragmentcredential",
    ] {
        assert!(!summary.contains(secret), "{secret} leaked into: {summary}");
    }
}

/// The authority ends at the first `/`, `?` or `#`, as a URL parser reads it,
/// so a `@` after one of them belongs to the path, query or fragment and the
/// summary prints the host the connection would use. An IPv6 host keeps its
/// brackets.
#[test]
fn the_pool_summary_reads_the_authority_the_way_a_url_does() {
    use codetrial::config::{Provider, pool_summary};

    for (url, shown) in [
        ("wss://[2001:db8::1]:7880", "[2001:db8::1]:7880"),
        ("wss://user:pw@[2001:db8::1]/room", "[2001:db8::1]"),
        ("wss://u:123#x@host", "u:123"),
        ("wss://host/path?mail=a@b", "host"),
    ] {
        let (summary, _) = pool_summary(&[Provider {
            id: "primary".to_string(),
            url: url.to_string(),
            api_key: "k".to_string(),
            api_secret: "s".to_string(),
            google_api_keys: vec!["g".to_string()],
        }]);
        assert!(
            summary.contains(&format!("primary={shown}")),
            "{url} -> {summary}"
        );
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
fn an_unreadable_end_sensitivity_falls_back_rather_than_reaching_the_wire() {
    let load = |value: &str| {
        load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "key"),
            ("LIVEKIT_API_SECRET", "secret"),
            ("GOOGLE_API_KEY", "google"),
            ("GEMINI_END_SENSITIVITY", value),
        ])
        .expect("complete config should load")
        .gemini_end_sensitivity
    };
    for low in ["LOW", "low", "END_SENSITIVITY_LOW"] {
        assert_eq!(load(low), "END_SENSITIVITY_LOW", "{low}");
    }
    for high in ["HIGH", "high", "END_SENSITIVITY_HIGH"] {
        assert_eq!(load(high), "END_SENSITIVITY_HIGH", "{high}");
    }
    for bad in ["END_SENSITIVITY_HIGHH", "medium", "  "] {
        assert_eq!(
            load(bad),
            DEFAULT_GEMINI_END_SENSITIVITY,
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
        "wss://example.livekit.cloud:443",
        "wss://[2001:db8::1]:7880",
    ] {
        assert!(codetrial::config::validate_livekit_url(url, true).is_ok());
    }
    // Prefix matching alone let all of these through.
    for url in [
        "wss://",
        "wss://@",
        "wss://project.example;script-src=*",
        "wss://project.example'",
        "wss://project.example,evil.example",
        "wss://project.example*",
        "wss://project.example:notaport",
        "wss://project.example:0",
        "wss://project.example:65536",
        "wss://[2001:db8::1",
        "wss://[not-ipv6]",
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
        google_api_keys: vec!["google".to_string()],
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
            "LIVEKIT_URL=wss://urluser:urlpassword@frankfurt.example:9443/privatepath?token=querycredential#fragmentcredential\nLIVEKIT_API_KEY=livekit-key\nLIVEKIT_API_SECRET=hunter2\nGOOGLE_API_KEY=gsecret\nGOOGLE_API_KEYS=list-secret-one,list-secret-two\n",
        )],
    );
    let (providers, _) = codetrial::config::discover_providers(&dir, false);

    let rendered = format!("{:?}", providers[0]);
    assert!(!rendered.contains("livekit-key"), "{rendered}");
    assert!(!rendered.contains("hunter2"), "{rendered}");
    assert!(!rendered.contains("gsecret"), "{rendered}");
    assert!(!rendered.contains("list-secret"), "{rendered}");
    for secret in [
        "urluser",
        "urlpassword",
        "privatepath",
        "querycredential",
        "fragmentcredential",
    ] {
        assert!(
            !rendered.contains(secret),
            "URL credential leaked: {rendered}"
        );
    }
    assert_eq!(
        providers[0].google_api_keys,
        ["list-secret-one", "list-secret-two"]
    );

    // The id and host make the line useful. Named against a host that is not
    // the id: this used to look for "eu" in a line that carried
    // `wss://eu.example`, so it held whether or not `Debug` printed the id at
    // all.
    assert!(rendered.contains("eu"), "{rendered}");
    assert!(rendered.contains("frankfurt.example"), "{rendered}");
}

/// The file's Gemini credential replaces the environment's under either name:
/// an inherited list outranking the file's key sent requests, and the bill,
/// to a project nobody configured for this deployment.
#[test]
fn a_config_file_credential_replaces_both_environment_names() {
    let base = [
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
    ];
    let merged = |env: &[(&str, &str)], file: &[(&str, &str)]| {
        let mut values = base
            .iter()
            .chain(env)
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<BTreeMap<_, _>>();
        codetrial::config::apply_config_file(
            &mut values,
            file.iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
        );
        load_from_pairs(values).unwrap()
    };

    let config = merged(
        &[("GOOGLE_API_KEYS", "env-a,env-b")],
        &[("GOOGLE_API_KEY", "file-key")],
    );
    assert_eq!(config.google_api_keys, ["file-key"]);

    let config = merged(
        &[("GOOGLE_API_KEY", "env-key")],
        &[("GOOGLE_API_KEYS", "file-a,file-b")],
    );
    assert_eq!(config.google_api_keys, ["file-a", "file-b"]);

    // A file without either name leaves the environment's credential alone.
    let config = merged(
        &[("GOOGLE_API_KEYS", "env-a,env-b")],
        &[("GEMINI_VOICE", "Puck")],
    );
    assert_eq!(config.google_api_keys, ["env-a", "env-b"]);

    // Nor does a blank line for the other name, which is no credential.
    let config = merged(&[("GOOGLE_API_KEY", "env-key")], &[("GOOGLE_API_KEYS", "")]);
    assert_eq!(config.google_api_keys, ["env-key"]);
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
    assert_eq!(config.pool.providers[0].google_api_keys, ["google"]);
}

/// The recording block, which is off in every deployment until an operator
/// provisions the resources in `docs/recording-contract.md`. These read the
/// parser directly rather than through `load_from_pairs`, because
/// `codetrial web` runs without a Gemini key and still has to validate this.
#[path = "config/recording.rs"]
mod recording;
#[path = "config/update_check.rs"]
mod update_check;

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

#[test]
fn compression_requires_a_valid_pair_and_preserves_provider_defaults() {
    let base = [
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", "google"),
    ];
    assert!(
        load_from_pairs(base)
            .unwrap()
            .gemini_context_compression
            .is_none()
    );
    let config = load_from_pairs(base.into_iter().chain([
        ("GEMINI_CONTEXT_TRIGGER_TOKENS", "25000"),
        ("GEMINI_CONTEXT_TARGET_TOKENS", "8000"),
    ]))
    .unwrap();
    let compression = config.gemini_context_compression.unwrap();
    assert_eq!(compression.trigger_tokens, 25000);
    assert_eq!(compression.target_tokens, 8000);
    for (trigger, target) in [
        ("25000", ""),
        ("", "8000"),
        ("0", "8000"),
        ("25000", "0"),
        ("8000", "8000"),
        ("7000", "8000"),
        ("oops", "8000"),
        ("4294967296", "8000"),
    ] {
        let error = load_from_pairs(base.into_iter().chain([
            ("GEMINI_CONTEXT_TRIGGER_TOKENS", trigger),
            ("GEMINI_CONTEXT_TARGET_TOKENS", target),
        ]))
        .err()
        .expect("invalid compression must fail configuration");
        assert!(!error.invalid_entries.is_empty());
    }
}

/// The message quotes the line that was refused, which is how an operator
/// finds it, not the one after it: the parser counts lines from 1 and the
/// quote is read back by that count.
#[test]
fn an_invalid_config_line_is_named_by_number_and_never_quoted() {
    let path =
        std::env::temp_dir().join(format!("codetrial-invalid-line-{}.env", std::process::id()));
    std::fs::write(
        &path,
        "# comment\nLIVEKIT_URL=wss://a\n  not a setting  \nNEXT=1\n",
    )
    .unwrap();
    let error = codetrial::config::read_config_file(&path).unwrap_err();
    let _ = std::fs::remove_file(&path);
    assert_eq!(
        error,
        format!("{}: line 3 is not KEY=value", path.display())
    );
    assert!(!error.contains("not a setting"), "{error}");
}

/// Notepad and VS Code can save UTF-8 with a byte order mark. One at the start
/// of the file is not part of the first key or comment, so the file reads as
/// it would without it, and a malformed line keeps its number.
#[test]
fn a_leading_byte_order_mark_is_not_part_of_the_first_line() {
    let read = |label: &str, text: &str| {
        let path =
            std::env::temp_dir().join(format!("codetrial-bom-{label}-{}.env", std::process::id()));
        std::fs::write(&path, text).unwrap();
        let result = codetrial::config::read_config_file(&path);
        let _ = std::fs::remove_file(&path);
        result.map_err(|error| error.replace(&path.display().to_string(), "<file>"))
    };

    for (label, text) in [
        ("key", "LIVEKIT_URL=wss://a\r\nLIVEKIT_API_KEY=key\r\n"),
        (
            "comment",
            "# Local configuration\r\nLIVEKIT_URL=wss://a\r\n",
        ),
        ("empty", ""),
        ("malformed", "LIVEKIT_URL=wss://a\nnot a setting\n"),
    ] {
        assert_eq!(
            read(&format!("{label}-marked"), &format!("\u{feff}{text}")),
            read(&format!("{label}-plain"), text),
            "{label}"
        );
    }
    assert_eq!(
        read("first-key", "\u{feff}LIVEKIT_URL=wss://a\n").unwrap(),
        [("LIVEKIT_URL".to_string(), "wss://a".to_string())]
    );
}

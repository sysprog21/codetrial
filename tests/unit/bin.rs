//! The `tests` module of `src/main.rs`, which declares this file by path.
//! Everything here reaches into `src/main.rs` through `super`, so it is a
//! unit test and not an integration test: private items are in scope.
//!
//! Named `bin.rs` rather than mirroring `main.rs` like its eighteen
//! siblings, because cargo reads `tests/<dir>/main.rs` as an integration
//! target and would compile this a second time as a crate root, where every
//! `super` in it is an error. `src/main.rs` says the same at the
//! declaration; it is repeated here because this is the file whose name
//! looks like a mistake.

fn values(pairs: &[(&str, &str)]) -> super::BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// A hostname, not a literal socket address. `bind_web_listener` hands the
/// string to `ToSocketAddrs`, which resolves it, and `CODETRIAL_WEB_ADDR`
/// has always accepted one. This used to be guarded in `config`, where the
/// value was parsed into a field only `serve` read; with the field gone,
/// the guard belongs on the one function that still takes the string.
#[test]
fn a_hostname_web_address_binds() {
    let listener = super::bind_web_listener("localhost:0").expect("a hostname address should bind");
    assert!(
        listener
            .local_addr()
            .expect("a bound listener has an address")
            .ip()
            .is_loopback()
    );
}

fn addr(text: &str) -> std::net::SocketAddr {
    text.parse().expect("test address should parse")
}

/// The published default is tolerated on loopback and nowhere else, and the
/// two unspecified addresses are the ones a container binds: neither is
/// loopback, and reading them as local is how the guard would miss the
/// deployment it exists for.
#[test]
fn the_published_session_secret_is_confined_to_loopback() {
    for local in [
        "127.0.0.1:3000",
        "127.0.0.53:3000",
        "[::1]:3000",
        // A dual-stack socket's spelling of the first one.
        "[::ffff:127.0.0.1]:3000",
    ] {
        assert_eq!(
            super::published_secret_refusal(&values(&[]), addr(local)),
            None,
            "{local} is loopback and needs no secret"
        );
    }

    for public in ["0.0.0.0:3000", "[::]:3000", "192.168.1.10:3000"] {
        let refusal = super::published_secret_refusal(&values(&[]), addr(public))
            .unwrap_or_else(|| panic!("{public} must be refused"));
        assert!(refusal.contains(public), "{refusal}");
        assert!(refusal.contains("SESSION_SECRET must be set"), "{refusal}");
    }
}

/// A secret of the operator's own is the whole point: setting it must lift
/// the restriction rather than merely change the message.
#[test]
fn a_real_session_secret_is_allowed_on_any_address() {
    assert_eq!(
        super::published_secret_refusal(
            &values(&[("SESSION_SECRET", "a-real-secret")]),
            addr("0.0.0.0:3000")
        ),
        None
    );
}

/// Naming the published value is not setting a secret. A guard that asked
/// only whether `SESSION_SECRET` was present would take the string this
/// repository publishes as proof that it is not being used, and whitespace
/// around it changes nothing, because the signer trims before it signs.
#[test]
fn spelling_out_the_published_default_does_not_count_as_setting_one() {
    for spelling in [super::DEFAULT_SESSION_SECRET, "  codetrial-local-session  "] {
        assert!(
            super::published_secret_refusal(
                &values(&[("SESSION_SECRET", spelling)]),
                addr("0.0.0.0:3000")
            )
            .is_some(),
            "{spelling:?} is the published key, not a secret"
        );
    }
}

/// A count that does not parse is not "no proxy": reading `one` as zero would
/// let a loopback bind behind a proxy pass for a local run.
#[test]
fn a_malformed_proxy_hop_count_is_refused_and_counts_as_a_proxy() {
    let bad = values(&[("CODETRIAL_TRUSTED_PROXY_HOPS", "one")]);
    assert!(matches!(
        super::reachable_from_elsewhere(&bad, addr("127.0.0.1:3000")),
        Some(super::Reachable::DeclaredProxy)
    ));
    let pool = codetrial::config::ProviderPool { providers: vec![] };
    let error = super::web_server_config(&bad, pool, false, &super::CliOptions::default())
        .expect_err("a malformed hop count must not start the server");
    assert!(error.contains("CODETRIAL_TRUSTED_PROXY_HOPS"), "{error}");
}

/// A bind address cannot see a reverse proxy. Declaring hops is the
/// operator saying requests reach this process from somewhere else, which
/// makes a loopback socket as public as the proxy in front of it.
#[test]
fn a_declared_proxy_makes_loopback_public_too() {
    let refusal = super::published_secret_refusal(
        &values(&[("CODETRIAL_TRUSTED_PROXY_HOPS", "1")]),
        addr("127.0.0.1:3000"),
    )
    .expect("loopback behind a declared proxy must be refused");
    assert!(refusal.contains("behind a declared proxy"), "{refusal}");

    assert_eq!(
        super::published_secret_refusal(
            &values(&[("CODETRIAL_TRUSTED_PROXY_HOPS", "0")]),
            addr("127.0.0.1:3000")
        ),
        None,
        "no declared proxy is the local run the default exists for"
    );
}

/// The precedence two launches depend on: a cold start builds this map
/// without a config file and has to reach the same address and the same
/// verdict `run_web` will reach with one. A flag overrides what was in the
/// environment; no flag leaves it alone.
#[test]
fn the_flags_override_the_environment_and_only_where_given() {
    let mut values = values(&[
        ("CODETRIAL_WEB_ADDR", "127.0.0.1:1"),
        ("CODETRIAL_WEB_DIR", "env-dir"),
        ("NODE_ENV", "production"),
    ]);
    super::apply_options(
        &mut values,
        &super::CliOptions {
            web_addr: Some("127.0.0.1:2".to_string()),
            room_prefix: Some("flag".to_string()),
            duration_min: Some(45),
            ..super::CliOptions::default()
        },
    );

    assert_eq!(values["CODETRIAL_WEB_ADDR"], "127.0.0.1:2");
    assert_eq!(values["CODETRIAL_ROOM_PREFIX"], "flag");
    assert_eq!(values["CODETRIAL_DURATION_MIN"], "45");
    assert_eq!(
        values["CODETRIAL_WEB_DIR"], "env-dir",
        "a flag that was not passed must not erase the environment"
    );
    assert_eq!(
        values["NODE_ENV"], "production",
        "and must not touch the rest"
    );
}

/// Setup is confined to the same interface the published session secret is,
/// and for a sharper reason: this listener hands `codetrial.env.local` to
/// whoever posts to it. Unlike `published_secret_refusal` there is no value
/// an operator can set to lift it, so every address is tested against one
/// empty environment.
#[test]
fn the_setup_page_is_confined_to_loopback() {
    for local in [
        "127.0.0.1:3000",
        "127.0.0.53:3000",
        "[::1]:3000",
        "[::ffff:127.0.0.1]:3000",
    ] {
        assert_eq!(
            super::public_setup_refusal(&values(&[]), addr(local)),
            None,
            "{local} reaches no further than this machine"
        );
    }

    for public in ["0.0.0.0:3000", "[::]:3000", "192.168.1.10:3000"] {
        let refusal = super::public_setup_refusal(&values(&[]), addr(public))
            .unwrap_or_else(|| panic!("{public} must be refused"));
        assert!(refusal.contains(public), "{refusal}");
        assert!(refusal.contains("codetrial.env.local"), "{refusal}");
    }
}

/// The same admission that makes a loopback bind public for the session
/// secret makes it public for Setup: a tunnel or an nginx in front is how
/// the internet reaches 127.0.0.1.
#[test]
fn a_declared_proxy_closes_the_setup_page_too() {
    let refusal = super::public_setup_refusal(
        &values(&[("CODETRIAL_TRUSTED_PROXY_HOPS", "1")]),
        addr("127.0.0.1:3000"),
    )
    .expect("loopback behind a declared proxy must be refused");

    // The variable by name: it is the only thing the reader of this message can
    // change, and the address they are on is already loopback.
    assert!(
        refusal.contains("CODETRIAL_TRUSTED_PROXY_HOPS"),
        "{refusal}"
    );

    assert_eq!(
        super::public_setup_refusal(
            &values(&[("CODETRIAL_TRUSTED_PROXY_HOPS", "0")]),
            addr("127.0.0.1:3000")
        ),
        None,
        "no declared proxy is the solo cold start this page exists for"
    );
}

/// The deployment this whole warning exists for: every credential set, and
/// `NODE_ENV` forgotten. Nothing else in the startup path says a word about
/// it, so if this stops firing the misconfiguration goes back to silent.
#[test]
fn a_start_without_node_env_says_what_it_relaxed() {
    let warnings = super::relaxed_for_local_use(&values(&[
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("GITHUB_CLIENT_ID", "id"),
    ]));

    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings[0].contains("NODE_ENV is not production"),
        "{warnings:?}"
    );
    assert!(
        warnings[1].contains("SESSION_SECRET is unset"),
        "{warnings:?}"
    );
    assert!(
        warnings[1].contains(super::DEFAULT_SESSION_SECRET),
        "the warning has to name the published value, or an operator cannot \
         tell which key is in use: {warnings:?}"
    );
}

/// A set secret drops that half and keeps the other. The relaxed CSP and
/// the plaintext LiveKit URL do not depend on the secret, so a local run
/// that sets one is still a local run.
#[test]
fn a_set_secret_leaves_only_the_mode_warning() {
    let warnings = super::relaxed_for_local_use(&values(&[("SESSION_SECRET", "a-real-secret")]));

    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("NODE_ENV is not production"),
        "{warnings:?}"
    );
}

/// `is_production` matches the exact string, so a capitalized value is not
/// production and the server really is running local defaults. Warning is
/// the correct answer, and it is the only thing that catches the typo.
#[test]
fn a_misspelled_node_env_is_not_production_and_says_so() {
    let warnings = super::relaxed_for_local_use(&values(&[
        ("NODE_ENV", "Production"),
        ("SESSION_SECRET", "a-real-secret"),
    ]));

    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("NODE_ENV is not production"),
        "{warnings:?}"
    );
}

/// Production is the configured case and says nothing. A warning on every
/// deployed start is a warning operators learn to scroll past, which is how
/// the one above stops working.
#[test]
fn production_warns_about_nothing() {
    assert!(super::relaxed_for_local_use(&values(&[("NODE_ENV", "production")])).is_empty());
    assert!(
        super::relaxed_for_local_use(&values(&[
            ("NODE_ENV", "production"),
            ("SESSION_SECRET", "a-real-secret"),
        ]))
        .is_empty()
    );
}

fn agent_config(pool: codetrial::config::ProviderPool) -> super::AgentConfig {
    let mut config = codetrial::config::load_from_pairs([
        ("LIVEKIT_URL", "wss://primary.example"),
        ("LIVEKIT_API_KEY", "primary-key"),
        ("LIVEKIT_API_SECRET", "primary-secret"),
        ("GOOGLE_API_KEY", "primary-google"),
    ])
    .unwrap();
    config.pool = pool;
    config
}

fn provider(id: &str) -> codetrial::config::Provider {
    codetrial::config::Provider {
        id: id.to_string(),
        url: format!("wss://{id}.example"),
        api_key: format!("{id}-key"),
        api_secret: format!("{id}-secret"),
        google_api_keys: vec![format!("{id}-google")],
    }
}

/// The whole point of writing the provider id into the room name: an agent
/// started separately has to land in the project the candidate was given a
/// token for, and has to say so when it cannot.
#[test]
fn a_room_name_routes_the_agent_to_the_provider_it_names() {
    let pool = codetrial::config::ProviderPool {
        providers: vec![
            provider(codetrial::config::PRIMARY_PROVIDER_ID),
            provider("eu"),
        ],
    };

    let config = super::select_provider(agent_config(pool.clone()), "interview-eu-a1b2c3d4")
        .expect("a configured provider should resolve");
    assert_eq!(config.livekit_url, "wss://eu.example");
    assert_eq!(config.livekit_api_secret, "eu-secret");
    assert_eq!(config.google_api_keys, ["eu-google"]);

    // No segment: what a single-provider deployment mints, and what a
    // hand-written INTERVIEW_ROOM_NAME usually looks like.
    let config = super::select_provider(agent_config(pool), "interview-a1b2c3d4").unwrap();
    assert_eq!(config.livekit_url, "wss://primary.example");
}

/// Falling back to the primary here is how an agent joins the wrong project
/// and sits in a room the candidate never appears in, for the whole
/// interview. A pool of one is exactly what a failed discovery produces, so
/// pool size is not evidence about the room.
#[test]
fn an_unconfigured_provider_id_is_refused_not_guessed() {
    for providers in [
        vec![provider(codetrial::config::PRIMARY_PROVIDER_ID)],
        vec![
            provider(codetrial::config::PRIMARY_PROVIDER_ID),
            provider("us"),
        ],
    ] {
        let pool = codetrial::config::ProviderPool { providers };
        // Not `expect_err`: `AgentConfig` deliberately has no `Debug`.
        let Err(error) =
            super::select_provider(agent_config(pool.clone()), "interview-eu-a1b2c3d4")
        else {
            panic!("an unknown provider id should refuse");
        };
        assert!(error.contains("eu"), "{error}");
    }
}

/// A `--config` that names nothing is an operator's mistake, which
/// `primary_config_path` reports by name. Reading it as a cold start
/// instead would answer a wrong path with a Setup page and write a second
/// config beside the one they meant. Both halves are asserted here because
/// the missing file is the only case where the two conditions disagree,
/// and it is a unit test rather than a spawned binary because a launch
/// that wrongly believes it is a cold start serves Setup and waits there.
#[test]
fn a_named_config_is_never_a_cold_start() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let named = |path: std::path::PathBuf| super::CliOptions {
        config_path: Some(path.display().to_string()),
        ..super::CliOptions::default()
    };

    let empty = values(&[]);
    assert!(
        !super::is_cold_start(&named(root.join("Cargo.toml")), &empty),
        "a config file that exists was named, so there is nothing to set up"
    );
    assert!(
        !super::is_cold_start(&named(root.join("no-such-config.env")), &empty),
        "a config file that is missing was still named, and a name is an \
         instruction to read that file rather than an invitation to ask"
    );
}

/// Exported credentials are a configured launch, so there is nothing to ask
/// for. Asking anyway put a page nobody would open in front of a headless
/// start that then waited forever, where naming the missing file and
/// exiting was something a log could carry.
#[test]
fn exported_credentials_are_a_configured_launch() {
    assert!(super::environment_supplies_credentials(&values(&[
        ("LIVEKIT_URL", "wss://example"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
    ])));

    // One short of the set the web side refuses to start without, which is
    // still a launch that has to be asked.
    assert!(!super::environment_supplies_credentials(&values(&[
        ("LIVEKIT_URL", "wss://example"),
        ("LIVEKIT_API_KEY", "key"),
    ])));

    // Present and blank is missing, the way `nonempty` reads it everywhere
    // else, and the way `read_config_file` would have written it back.
    assert!(!super::environment_supplies_credentials(&values(&[
        ("LIVEKIT_URL", "wss://example"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "   "),
    ])));

    // The optional one is not part of the question.
    assert!(super::environment_supplies_credentials(&values(&[
        ("LIVEKIT_URL", "wss://example"),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
        ("GOOGLE_API_KEY", ""),
    ])));
}

#[test]
fn bare_config_paths_discover_providers_from_the_current_directory() {
    let options = super::CliOptions {
        config_path: Some("prod.env".to_string()),
        ..Default::default()
    };
    assert_eq!(super::config_dir(&options), std::path::PathBuf::from("."));
}

/// The database is the config directory's, so a named config carries it
/// along rather than leaving it wherever the process was started.
#[test]
fn the_account_database_sits_beside_the_config_that_was_named() {
    let options = super::CliOptions {
        config_path: Some("/etc/codetrial/prod.env".to_string()),
        ..Default::default()
    };
    assert_eq!(
        super::account_db_path(&values(&[]), &options),
        std::path::PathBuf::from("/etc/codetrial/codetrial.db")
    );
    assert_eq!(
        super::account_db_path(
            &values(&[("CODETRIAL_DB_PATH", "/srv/accounts.db")]),
            &options
        ),
        std::path::PathBuf::from("/srv/accounts.db")
    );
}

/// The pair, not the id. `login_config` enables OAuth only when it has
/// both, so an id with no secret verifies exactly as many people as no id
/// at all, and would have started a server that refused every interview
/// while looking configured.
#[test]
fn a_service_account_that_cannot_sign_stops_the_server() {
    // Not at the first delivery, which is an hour into somebody's interview,
    // and not as a warning that leaves the server running with no way to hand a
    // recording over.
    let mut recording = codetrial::config::RecordingConfig {
        livekit: None,
        gcs_bucket: "codetrial-staging".to_string(),
        gcs_prefix: "codetrial".to_string(),
        drive_id: "0AKfixtureDriveId".to_string(),
        service_account_json: r#"{"client_email":"a@b.iam.gserviceaccount.com","private_key":"-----BEGIN PRIVATE KEY-----
bm90IGEga2V5
-----END PRIVATE KEY-----
"}"#.to_string(),
        max_minutes: 45,
        bitrate: 2000,
        kill_switch: false,
        template_base_url: "https://recording.example".to_string(),
    };
    let error = super::recording_can_deliver(Some(&recording)).unwrap_err();
    assert!(
        error.contains("CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON"),
        "{error}"
    );

    // And a deployment that records nothing needs no credential at all.
    assert!(super::recording_can_deliver(None).is_ok());
    recording.service_account_json = String::new();
    assert!(super::recording_can_deliver(Some(&recording)).is_err());
}

fn recording_values(pairs: &[(&str, &str)]) -> super::BTreeMap<String, String> {
    let mut values: std::collections::BTreeMap<String, String> = [
        ("CODETRIAL_RECORDING_ENABLED", "true"),
        ("CODETRIAL_RECORDING_GCS_BUCKET", "codetrial-staging"),
        ("CODETRIAL_RECORDING_DRIVE_ID", "0AKfixtureDriveId"),
        (
            "CODETRIAL_RECORDING_SERVICE_ACCOUNT_JSON",
            r#"{"type":"service_account","client_email":"a@b.iam.gserviceaccount.com","private_key":"KEYMATERIAL-4bd2"}"#,
        ),
        (
            "CODETRIAL_RECORDING_TEMPLATE_BASE_URL",
            "https://recording.codetrial.example",
        ),
        ("SESSION_SECRET", "a-real-secret"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    for (key, value) in pairs {
        values.insert(key.to_string(), value.to_string());
    }
    values
}

/// The recording check sees the length the endpoints grant, not the raw text:
/// `120` is served as 90 so a cap of 90 holds it, and a length left empty is
/// served as 45 so a cap below that is refused.
#[test]
fn the_recording_cap_is_checked_against_the_clamped_length() {
    let pool = codetrial::config::ProviderPool {
        providers: vec![provider(codetrial::config::PRIMARY_PROVIDER_ID)],
    };
    let refusal = |duration: &str, cap: &str| {
        let values = recording_values(&[
            ("CODETRIAL_DURATION_MIN", duration),
            ("CODETRIAL_RECORDING_MAX_MINUTES", cap),
        ]);
        super::web_server_config(&values, pool.clone(), false, &super::CliOptions::default()).err()
    };

    // Past the cap check the fixture still lacks an OAuth app, so what matters
    // is only whether the refusal is about the cap.
    let about_the_cap = |error: &Option<String>| {
        error
            .as_deref()
            .is_some_and(|text| text.contains("CODETRIAL_RECORDING_MAX_MINUTES"))
    };
    assert!(!about_the_cap(&refusal("120", "90")));
    assert!(refusal("120", "89").unwrap().contains("89, below the 90"));
    assert!(refusal("", "44").unwrap().contains("44, below the 45"));
    assert!(!about_the_cap(&refusal("", "45")));
}

/// Half an OAuth app is not an OAuth app.
///
/// The rule below is tested on its own; this is the wiring that feeds it,
/// and the two lookups have to be joined with "and". Joined with "or", a
/// deployment holding only a client id would start recording and then
/// refuse every interview at `/api/token`, because the address a recording
/// is delivered to is the one GitHub returns and there is no GitHub. Each
/// half alone has to be refused, and so does neither.
#[test]
fn recording_starts_only_with_both_halves_of_the_oauth_app() {
    let pool = codetrial::config::ProviderPool {
        providers: vec![provider(codetrial::config::PRIMARY_PROVIDER_ID)],
    };

    for half in [
        vec![("GITHUB_CLIENT_ID", "id")],
        vec![("GITHUB_CLIENT_SECRET", "secret")],
        vec![],
    ] {
        let error = super::web_server_config(
            &recording_values(&half),
            pool.clone(),
            false,
            &super::CliOptions::default(),
        )
        .expect_err("recording with half an OAuth app must not start");
        assert!(error.contains("GITHUB_CLIENT_ID"), "{error}");
    }

    // The other direction is `recording_needs_verified_identity` below, which
    // takes the answer rather than the lookups: reaching it from here means a
    // service account whose key really parses, and the rule it applies is the
    // same either way.
    let _ = pool;
}

#[test]
fn recording_needs_both_oauth_credentials_or_it_refuses_to_start() {
    assert!(super::recording_needs_verified_identity(false, false).is_ok());
    assert!(super::recording_needs_verified_identity(true, true).is_ok());

    let error = super::recording_needs_verified_identity(true, false).unwrap_err();
    assert!(error.contains("GITHUB_CLIENT_ID"));
    assert!(error.contains("GITHUB_CLIENT_SECRET"));
}

#[test]
fn guest_mode_setting_respects_environment_values() {
    assert!(!super::guest_mode_enabled(&values(&[])));
    assert!(super::guest_mode_enabled(&values(&[(
        "CODETRIAL_GUEST_MODE_ENABLED",
        "true"
    ),])));
    assert!(!super::guest_mode_enabled(&values(&[(
        "CODETRIAL_GUEST_MODE_ENABLED",
        "false"
    ),])));
}

#[test]
fn guest_mode_recording_conflict_requires_both_settings() {
    assert!(!super::guest_mode_recording_conflict(false, false));
    assert!(!super::guest_mode_recording_conflict(false, true));
    assert!(!super::guest_mode_recording_conflict(true, false));
    assert!(super::guest_mode_recording_conflict(true, true));
}

/// Written by hand in a config file or by a launcher, so the spacing and a
/// trailing comma are the writer's, and neither may become a host name that
/// matches a request with an empty `Host`.
#[test]
fn the_allowed_hosts_are_a_trimmed_list_without_blanks() {
    assert!(super::allowed_hosts(&values(&[])).unwrap().is_empty());
    assert_eq!(
        super::allowed_hosts(&values(&[(
            "CODETRIAL_ALLOWED_HOSTS",
            " 127.0.0.1:3101 ,, Desktop.Test ,"
        )]))
        .unwrap(),
        ["127.0.0.1:3101", "Desktop.Test"]
    );
}

/// A list that parses to nothing would switch the check off while the person
/// who wrote it believes every other name is refused.
#[test]
fn an_allowed_hosts_list_with_no_host_is_refused_not_read_as_off() {
    // Nothing, or only spacing, is an unset variable, as for every other
    // setting.
    for unset in ["", " "] {
        assert!(
            super::allowed_hosts(&values(&[("CODETRIAL_ALLOWED_HOSTS", unset)]))
                .unwrap()
                .is_empty(),
            "{unset:?}"
        );
    }
    for blank in [" , ", ",,"] {
        assert!(
            super::allowed_hosts(&values(&[("CODETRIAL_ALLOWED_HOSTS", blank)])).is_err(),
            "{blank:?}"
        );
    }
    let argv = ["web", "--allowed-hosts", " "].map(String::from);
    assert!(super::parse_agent_args(&argv).is_err());
}

/// Every path a launch derives, from the one folder `--data-dir` names. The
/// CLI's own answers come from the working directory and the executable's
/// folder, which a desktop launcher does not choose, so none of them may leak
/// through.
#[test]
fn a_data_dir_decides_every_path_a_launch_reads_or_writes() {
    // A folder nothing has made, so the missing config below is this test's
    // doing and not whatever the host happens to keep at a fixed path.
    let data = std::env::temp_dir().join(format!(
        "codetrial-bin-data-dir-{}-{}",
        std::process::id(),
        codetrial::current_epoch_millis()
    ));
    let data = data.as_path();
    let options = super::CliOptions {
        data_dir: Some(data.to_path_buf()),
        ..Default::default()
    };
    assert_eq!(super::config_dir(&options), data);
    assert_eq!(
        super::cold_start_config_path(&options),
        data.join("codetrial.env.local")
    );
    assert_eq!(
        super::web_error_log_path(&options),
        data.join("codetrial-error.log")
    );
    assert_eq!(
        super::account_db_path(&values(&[]), &options),
        data.join("codetrial.db")
    );
    let missing = super::primary_config_path(&options).unwrap_err();
    assert!(
        missing.contains(&data.join("codetrial.env.local").display().to_string()),
        "{missing}"
    );

    let pool = codetrial::config::ProviderPool {
        providers: vec![provider(codetrial::config::PRIMARY_PROVIDER_ID)],
    };
    let served = |values: &super::BTreeMap<String, String>| {
        super::web_server_config(values, pool.clone(), false, &options)
            .expect("a plain local config should be accepted")
            .web_dir
    };
    assert_eq!(served(&values(&[])), data.join("web"));
    assert_eq!(
        served(&values(&[("CODETRIAL_WEB_DIR", "/elsewhere")])),
        std::path::Path::new("/elsewhere")
    );
}

/// The operator's `CODETRIAL_DURATION_MIN` is what the lobby and `/api/token`
/// default to, so it has to reach the web config.
#[test]
fn web_server_config_carries_the_configured_default_length() {
    let options = super::CliOptions::default();
    let pool = codetrial::config::ProviderPool {
        providers: vec![provider(codetrial::config::PRIMARY_PROVIDER_ID)],
    };
    let configured = |values: &super::BTreeMap<String, String>| {
        super::web_server_config(values, pool.clone(), false, &options)
            .expect("a plain local config should be accepted")
            .default_duration_min
    };
    assert_eq!(
        configured(&values(&[])),
        codetrial::config::DEFAULT_DURATION_MIN
    );
    assert_eq!(configured(&values(&[("CODETRIAL_DURATION_MIN", "30")])), 30);
    assert_eq!(
        configured(&values(&[("CODETRIAL_DURATION_MIN", " 30 ")])),
        30
    );
    // A length typed as text is refused, not read as the built-in one.
    let error = super::web_server_config(
        &values(&[("CODETRIAL_DURATION_MIN", "3O")]),
        pool.clone(),
        false,
        &options,
    )
    .expect_err("a length that is no number must not start the server");
    assert!(error.contains("CODETRIAL_DURATION_MIN"), "{error}");
}

/// Made once and kept, so a relaunch keeps everyone signed in, and owner-only
/// because any program on the machine can reach a loopback port.
#[test]
fn a_data_dir_session_key_is_made_once_and_kept() {
    let dir = std::env::temp_dir().join(format!(
        "codetrial-bin-secret-{}-{}",
        std::process::id(),
        codetrial::current_epoch_millis()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(super::SESSION_SECRET_FILE);

    let first = super::data_dir_session_secret(&path).unwrap();
    assert!(first.len() >= 32, "{first}");
    assert_ne!(first, super::DEFAULT_SESSION_SECRET);
    assert_eq!(super::data_dir_session_secret(&path).unwrap(), first);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{mode:o}");
    }

    // A key of every character this program writes, `-` and `_` among them, is
    // kept as it is.
    let written = "ab-CD_01".repeat(5);
    std::fs::write(&path, &written).unwrap();
    assert_eq!(super::data_dir_session_secret(&path).unwrap(), written);

    // Refused, and left as it was, rather than replaced.
    for damaged in ["", "short", "codetrial-local-session", &"=".repeat(40)] {
        std::fs::write(&path, damaged).unwrap();
        let error = super::data_dir_session_secret(&path).unwrap_err();
        assert!(error.contains("not a session key"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), damaged);
    }

    // Unreadable is not missing: a key is made only where there is none.
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let error = super::data_dir_session_secret(&path).unwrap_err();
    assert!(error.starts_with("cannot read"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The flag a launcher passes, since a `--data-dir` launch reads no
/// configuration from its environment, and the environment it would have read.
#[test]
fn a_data_dir_launch_takes_its_settings_from_flags_not_the_environment() {
    // Parsed from the command line the desktop app passes, so the flags are
    // proved to reach the options as well as to be read from them.
    let argv = [
        "web",
        "--data-dir",
        "/data",
        "--allowed-hosts",
        "127.0.0.1:3101",
    ]
    .map(String::from);
    let (_, options) = super::parse_agent_args(&argv).unwrap();
    assert_eq!(options.data_dir, Some(std::path::PathBuf::from("/data")));
    assert!(super::environment(&options).is_empty());
    assert_eq!(
        super::allowed_hosts(&super::environment_and_flags(&options)).unwrap(),
        ["127.0.0.1:3101"]
    );
}

#[test]
fn task_mode_is_asked_for_by_a_flag_that_takes_no_value() {
    let parse = |argv: &[&str]| {
        super::parse_agent_args(&argv.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
    };
    let (positionals, options) = parse(&["web"]).unwrap();
    assert_eq!(
        (positionals, options.task_mode),
        (vec!["web".to_string()], false)
    );
    // The word after the flag is a positional, not its value.
    let (positionals, options) = parse(&["--task-mode", "web"]).unwrap();
    assert_eq!(
        (positionals, options.task_mode),
        (vec!["web".to_string()], true)
    );
    let (_, options) = parse(&["web", "--task-mode", "--data-dir", "/data"]).unwrap();
    assert!(options.task_mode);
    assert_eq!(options.data_dir, Some(std::path::PathBuf::from("/data")));
    // Refused for what it is, not as an unknown flag.
    assert_eq!(
        parse(&["web", "--task-mode=yes"]).unwrap_err(),
        "--task-mode takes no value"
    );
}

/// The desktop app passes `--no-update-check`, and it has to win over a
/// config file that turned the check on, which is the one it imported.
#[test]
fn no_update_check_is_a_flag_that_turns_the_setting_off() {
    let parse = |argv: &[&str]| {
        super::parse_agent_args(&argv.iter().map(|arg| arg.to_string()).collect::<Vec<_>>())
    };
    let (_, options) = parse(&["web"]).unwrap();
    assert!(!options.no_update_check);
    let (positionals, options) = parse(&["--no-update-check", "web"]).unwrap();
    assert_eq!(
        (positionals, options.no_update_check),
        (vec!["web".to_string()], true)
    );
    assert_eq!(
        parse(&["web", "--no-update-check=yes"]).unwrap_err(),
        "--no-update-check takes no value"
    );

    let mut on = values(&[("CODETRIAL_UPDATE_CHECK", "true")]);
    super::apply_options(&mut on, &super::CliOptions::default());
    assert_eq!(on["CODETRIAL_UPDATE_CHECK"], "true");
    super::apply_options(
        &mut on,
        &super::CliOptions {
            no_update_check: true,
            ..super::CliOptions::default()
        },
    );
    assert_eq!(on["CODETRIAL_UPDATE_CHECK"], "false");
}

/// A refusal says what to change, and each reason names its own cause.
#[test]
fn task_mode_says_why_it_may_not_run_here() {
    assert_eq!(super::task_mode_refusal(None, false, false), None);
    let reasons = [
        super::task_mode_refusal(Some(super::Reachable::Address), false, false).unwrap(),
        super::task_mode_refusal(Some(super::Reachable::DeclaredProxy), false, false).unwrap(),
        super::task_mode_refusal(None, true, false).unwrap(),
        super::task_mode_refusal(None, false, true).unwrap(),
    ];
    for (reason, needle) in reasons.iter().zip([
        "--web-addr",
        "CODETRIAL_TRUSTED_PROXY_HOPS",
        "INTERVIEW_ROOM_NAME",
        "recording",
    ]) {
        assert!(reason.contains(needle), "{reason}");
    }
    // The first cause that applies is the one said.
    assert_eq!(
        super::task_mode_refusal(Some(super::Reachable::Address), true, true),
        Some(reasons[0])
    );
    assert_eq!(super::task_mode_refusal(None, true, true), Some(reasons[2]));
}

#[test]
fn task_mode_needs_the_flag_and_a_server_it_may_run_on() {
    assert!(super::task_mode_enabled(true, None, false, false));

    // Asking is not enough on a server it may not run on, and a server it may
    // run on is not enough without asking.
    assert!(!super::task_mode_enabled(false, None, false, false));
    for (reachable, fixed_room, recording) in [
        (Some(super::Reachable::Address), false, false),
        (Some(super::Reachable::DeclaredProxy), false, false),
        (None, true, false),
        (None, false, true),
    ] {
        assert!(!super::task_mode_enabled(
            true, reachable, fixed_room, recording
        ));
    }
}

#[test]
fn task_mode_runs_only_on_a_private_server_that_neither_records_nor_shares_a_room() {
    assert!(super::task_mode_allowed(None, false, false));
    for (reachable, fixed_room, recording) in [
        (Some(super::Reachable::Address), false, false),
        (Some(super::Reachable::DeclaredProxy), false, false),
        (None, true, false),
        (None, false, true),
    ] {
        assert!(!super::task_mode_allowed(reachable, fixed_room, recording));
    }
}

#[test]
fn peer_is_offered_once_oauth_and_observer_keys_exist() {
    let oauth = |settings: &[(&str, &str)]| {
        codetrial::launch::peer_services(&values(settings), true).unwrap()
    };
    assert!(oauth(&[]).is_none());
    assert!(oauth(&[("GOOGLE_API_KEY", "test-key")]).is_some());
    assert!(
        codetrial::launch::peer_services(&values(&[("GOOGLE_API_KEY", "test-key")]), false)
            .unwrap()
            .is_none()
    );
    let pool = codetrial::config::ProviderPool {
        providers: vec![provider(codetrial::config::PRIMARY_PROVIDER_ID)],
    };
    let build = |settings: &[(&str, &str)]| {
        super::web_server_config(
            &values(settings),
            pool.clone(),
            false,
            &super::CliOptions::default(),
        )
        .unwrap()
    };
    let credentials = [
        ("GITHUB_CLIENT_ID", "client"),
        ("GITHUB_CLIENT_SECRET", "secret"),
    ];
    assert!(build(&credentials).peer.is_none());
    let full = [
        credentials[0],
        credentials[1],
        ("GOOGLE_API_KEY", "test-key"),
    ];
    let offered = build(&full);
    assert!(offered.peer.is_some() && offered.peer_observer.is_some());
    assert!(build(&[("GOOGLE_API_KEY", "test-key")]).peer.is_none());
    for half in credentials {
        assert!(
            build(&[half, ("GOOGLE_API_KEY", "test-key")])
                .peer
                .is_none(),
            "{} alone is not GitHub sign-in",
            half.0
        );
    }
}

#[test]
fn peer_observer_rejects_sessions_longer_than_its_two_hour_capture() {
    let long = values(&[
        ("GOOGLE_API_KEY", "test-key"),
        ("CODETRIAL_PEER_HARD_MAX_MINUTES", "150"),
    ]);
    assert!(
        codetrial::launch::peer_services(&long, true)
            .unwrap_err()
            .contains("at most 120")
    );
    let limit = values(&[
        ("GOOGLE_API_KEY", "test-key"),
        ("CODETRIAL_PEER_HARD_MAX_MINUTES", "120"),
    ]);
    assert!(codetrial::launch::peer_services(&limit, true).is_ok());
}

/// A folder with an empty config file in it and values pointing the account
/// database into it, so `prepare` touches nothing outside the test.
fn launch_fixture(label: &str) -> (std::path::PathBuf, super::CliOptions) {
    let dir = std::env::temp_dir().join(format!(
        "codetrial-bin-launch-{label}-{}-{}",
        std::process::id(),
        codetrial::current_epoch_millis()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("codetrial.env.local");
    std::fs::write(&config, "").unwrap();
    let options = super::CliOptions {
        config_path: Some(config.display().to_string()),
        ..Default::default()
    };
    (dir, options)
}

fn livekit_values(dir: &std::path::Path) -> super::BTreeMap<String, String> {
    let db = dir.join("accounts.db").display().to_string();
    values(&[
        ("LIVEKIT_URL", "wss://primary.example"),
        ("LIVEKIT_API_KEY", "primary-key"),
        ("LIVEKIT_API_SECRET", "primary-secret"),
        ("CODETRIAL_DB_PATH", &db),
    ])
}

#[test]
fn prepare_builds_the_server_config_and_opens_the_account_database() {
    let (dir, options) = launch_fixture("prepare");
    let prepared = super::prepare(&livekit_values(&dir), &options).unwrap();
    assert_eq!(prepared.config.pool.providers.len(), 1);
    assert!(prepared.max_concurrent > 0);
    assert!(dir.join("accounts.db").is_file(), "the database is opened");
    assert!(
        prepared.config.tasks.is_none(),
        "task mode waits for the bind"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prepare_refuses_a_launch_without_livekit_credentials() {
    let (dir, options) = launch_fixture("prepare-missing");
    let error = super::prepare(&values(&[]), &options).err().unwrap();
    assert!(
        error.contains("missing required LiveKit credentials"),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn finish_refuses_the_published_secret_on_a_public_address() {
    let (dir, options) = launch_fixture("finish-public");
    let values = livekit_values(&dir);
    let prepared = super::prepare(&values, &options).unwrap();
    let refusal = super::finish(
        prepared.config,
        &values,
        &options,
        addr("203.0.113.9:3000"),
        super::LaunchedBy::Operator,
    )
    .err()
    .unwrap();
    assert!(!refusal.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn finish_offers_a_launchers_child_no_setup_page_and_no_task_mode() {
    let (dir, options) = launch_fixture("finish-child");
    let mut values = livekit_values(&dir);
    values.insert(
        "SESSION_SECRET".into(),
        "a-real-secret-for-this-test-only-0123".into(),
    );
    let prepared = super::prepare(&values, &options).unwrap();
    let (config, setup) = super::finish(
        prepared.config,
        &values,
        &options,
        addr("127.0.0.1:3000"),
        super::LaunchedBy::Parent,
    )
    .unwrap();
    assert!(setup.is_none(), "a child has no terminal anyone reads");
    assert!(config.tasks.is_none(), "task mode was not asked for");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_local_interviewer_needs_a_gemini_key_and_a_valid_config() {
    let (dir, _) = launch_fixture("agent");
    let mut values = livekit_values(&dir);
    assert!(super::local_agent_config(values.clone()).unwrap().is_none());
    values.insert("GOOGLE_API_KEY".into(), "google-key".into());
    assert!(super::local_agent_config(values.clone()).unwrap().is_some());
    values.insert("GEMINI_CONTEXT_TRIGGER_TOKENS".into(), "1000".into());
    let error = super::local_agent_config(values).err().unwrap();
    assert!(error.contains("GEMINI_CONTEXT"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A dispatcher that says whether an interview is still running or writing its
/// report, notes that admission was closed, and starts nothing.
struct Busy {
    busy: std::sync::atomic::AtomicBool,
    closed: std::sync::atomic::AtomicBool,
}

impl codetrial::web::RoomDispatcher for Busy {
    fn ensure_agent(
        &self,
        _: &str,
        _: &codetrial::config::Provider,
        _: codetrial::web::AgentJob,
    ) -> Result<(), codetrial::web::DispatchRefusal> {
        Ok(())
    }

    fn is_idle(&self) -> bool {
        !self.busy.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn close_admission(&self) {
        self.closed.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Serves with a stop that has already happened and a dispatcher that is busy
/// for `busy_for` (`None`: forever), and returns how the run ended, how long it
/// took and whether admission had been closed.
fn run_stopped(
    label: &str,
    busy_for: Option<std::time::Duration>,
    settle: Option<std::time::Duration>,
) -> (Result<(), String>, std::time::Duration, bool) {
    let (dir, options) = launch_fixture(label);
    let values = livekit_values(&dir);
    let prepared = super::prepare(&values, &options).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let bound = listener.local_addr().unwrap();
    let (config, setup) = super::finish(
        prepared.config,
        &values,
        &options,
        bound,
        super::LaunchedBy::Parent,
    )
    .unwrap();
    let dispatcher = std::sync::Arc::new(Busy {
        busy: std::sync::atomic::AtomicBool::new(true),
        closed: std::sync::atomic::AtomicBool::new(false),
    });
    if let Some(after) = busy_for {
        let dispatcher = std::sync::Arc::clone(&dispatcher);
        std::thread::spawn(move || {
            std::thread::sleep(after);
            dispatcher
                .busy
                .store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }
    let serving = std::sync::Arc::clone(&dispatcher);
    let began = std::time::Instant::now();

    // On its own thread with a deadline, so a bound that stops working fails
    // this test instead of hanging the suite.
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(super::serve_with(
            listener,
            config,
            setup,
            Some(serving),
            async {},
            super::Bounds {
                drain: None,
                settle,
            },
        ));
    });
    let result = finished
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("serve_with returned within its bounds");
    let elapsed = began.elapsed();
    let _ = std::fs::remove_dir_all(&dir);
    let closed = dispatcher.closed.load(std::sync::atomic::Ordering::SeqCst);
    (result, elapsed, closed)
}

#[test]
fn a_stopping_server_waits_for_its_rooms_and_reports_to_finish() {
    let (result, elapsed, closed) = run_stopped(
        "settle-waits",
        Some(std::time::Duration::from_millis(600)),
        Some(std::time::Duration::from_secs(20)),
    );
    result.unwrap();
    assert!(
        closed,
        "nothing new is admitted once the shutdown has begun"
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(500),
        "{elapsed:?}"
    );
}

#[test]
fn rooms_still_running_at_the_settle_bound_are_reported_not_hidden() {
    let (result, elapsed, closed) = run_stopped(
        "settle-bound",
        None,
        Some(std::time::Duration::from_millis(300)),
    );
    assert!(closed);
    let error = result.unwrap_err();
    assert!(error.contains("settle bound"), "{error}");
    // It waited out the bound, and no longer.
    assert!(
        elapsed >= std::time::Duration::from_millis(250),
        "{elapsed:?}"
    );
    assert!(elapsed < std::time::Duration::from_secs(10), "{elapsed:?}");
}

#[test]
fn without_a_settle_bound_the_product_stops_without_waiting() {
    let (result, elapsed, closed) = run_stopped("settle-none", None, None);
    assert!(
        closed,
        "the product closes admission as its shutdown begins too"
    );
    result.unwrap();
    assert!(elapsed < std::time::Duration::from_secs(5), "{elapsed:?}");
}

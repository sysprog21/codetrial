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
        google_api_key: format!("{id}-google"),
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
    assert_eq!(config.google_api_key, "eu-google");

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
    let recording_values = |pairs: &[(&str, &str)]| {
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
    };
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

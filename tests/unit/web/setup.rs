//! The `tests` module of `src/web/setup.rs`, which declares this file by path.
//! Everything here reaches into `src/web/setup.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::{host_is_loopback, read_offer};
use axum::http::HeaderValue;
use std::path::PathBuf;

fn host(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).expect("a test Host should be a header value")
}

#[test]
fn a_loopback_host_naming_the_bound_port_is_accepted() {
    for value in [
        "127.0.0.1:3000",
        "localhost:3000",
        "[::1]:3000",
        "LocalHost:3000",
    ] {
        assert!(host_is_loopback(Some(&host(value)), 3000), "{value}");
    }
}

/// The rebinding case: the name is the attacker's, the port is ours, and
/// the port is the half that matches.
#[test]
fn a_host_that_is_not_a_loopback_name_is_refused() {
    for value in [
        "codetrial.example:3000",
        "127.0.0.1.codetrial.example:3000",
        "localhost.codetrial.example:3000",
    ] {
        assert!(!host_is_loopback(Some(&host(value)), 3000), "{value}");
    }
}

/// A `Host` is only missing or portless from something that is not the
/// browser this page was opened in, since the console printed the port.
#[test]
fn a_missing_or_mismatched_port_is_refused() {
    assert!(!host_is_loopback(None, 3000));
    assert!(!host_is_loopback(Some(&host("127.0.0.1")), 3000));
    assert!(!host_is_loopback(Some(&host("127.0.0.1:3001")), 3000));
    assert!(!host_is_loopback(Some(&host("[::1]")), 3000));
    assert!(!host_is_loopback(Some(&host("127.0.0.1:")), 3000));
}

/// Port 80 is the one a browser leaves out of the `Host`, so it is the one
/// case where a portless value is the real thing rather than a hand-written
/// request.
#[test]
fn the_default_http_port_is_accepted_without_one() {
    assert!(host_is_loopback(Some(&host("localhost")), 80));
    assert!(host_is_loopback(Some(&host("localhost:80")), 80));
    assert!(!host_is_loopback(Some(&host("localhost")), 3000));

    // The bracket guard's case: the last colon of `[::1]` is inside the
    // brackets, and reading it as the separator refuses a real browser.
    assert!(host_is_loopback(Some(&host("[::1]")), 80));
}

/// The session sweeper is a timer, and a timer needs a runtime.
///
/// Both halves matter. Inside one it has to actually start, because the point
/// of the change is that reclaiming expired sessions stopped being a single
/// pass at boot; outside one it has to decline rather than panic, because
/// `web_router` is public and a router built without a runtime is degraded
/// rather than broken -- the rule `spawn_recording_workers` already follows.
///
/// What each tick does is `sweep_sessions_once`, covered against a real
/// database in tests/unit/accounts/schema.migration_tests.rs.
#[test]
fn the_session_sweeper_starts_under_a_runtime_and_declines_without_one() {
    let scratch = AccountsScratch::new("sweeper");

    assert!(
        crate::web::spawn_session_sweeper(scratch.accounts.clone()).is_empty(),
        "no runtime to spawn on, so no timer and no panic"
    );

    // That dropping it stops it is the shared type's promise, and is held once,
    // in tests/unit/web/recordings/workers.rs.
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let sweeper =
        runtime.block_on(async { crate::web::spawn_session_sweeper(scratch.accounts.clone()) });
    let handle = sweeper.0.first().expect("a runtime means a timer");
    assert!(!handle.is_finished(), "it runs for the life of the server");
}

/// What each tick of the sweeper does, observed rather than assumed.
///
/// The timer test above proves a sweeper starts; nothing proved it sweeps.
/// Replacing `sweep_sessions_once` with an empty body left every test green,
/// which the mutation lane reported, so a server could have run for a month
/// on a timer that woke hourly and deleted nothing. An expired session and a
/// live one go in, and only the expired one comes out.
#[test]
fn one_sweep_reclaims_what_has_expired_and_nothing_else() {
    let scratch = AccountsScratch::new("sweep-once");
    let accounts = &scratch.accounts;
    accounts
        .with(|connection| {
            connection.execute_batch(
                "
                INSERT INTO users (id, github_id, login, created_at, updated_at)
                  VALUES (-1, -1, 'gone', 1, 1);
                INSERT INTO users (id, github_id, login, created_at, updated_at)
                  VALUES (-2, -2, 'live', 1, 1);
                INSERT INTO sessions (id, user_id, expires_at, created_at)
                  VALUES ('expired', -1, 100, 1);
                INSERT INTO sessions (id, user_id, expires_at, created_at)
                  VALUES ('live', -2, 9999999999, 1);
                ",
            )
        })
        .unwrap();

    crate::web::sweep_sessions_once(accounts);

    let remaining: Vec<String> = accounts
        .with(|connection| {
            let mut statement = connection.prepare("SELECT id FROM sessions ORDER BY id")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect()
        })
        .unwrap();
    assert_eq!(remaining, ["live"]);
}

/// An hour, and pinned, because the two ways the arithmetic can slip are not
/// equally harmless. Two minutes is merely wasteful; one second is a loop that
/// takes the accounts connection's lock every second for the life of the
/// server, contending with every sign-in for it.
#[test]
fn the_sweeper_wakes_hourly() {
    assert_eq!(
        crate::web::SESSION_SWEEP_INTERVAL,
        std::time::Duration::from_secs(3600)
    );
}

/// A throwaway accounts database for one test, opened the way the server opens
/// it and removed when the test ends. Removed on drop rather than by a line at
/// the end of each test, because a test that ends by panicking is exactly the
/// one whose leftovers would otherwise stay behind.
struct AccountsScratch {
    dir: std::path::PathBuf,
    accounts: std::sync::Arc<crate::accounts::Accounts>,
}

impl AccountsScratch {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "codetrial-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let accounts = crate::web::open_accounts(crate::web::GitHubLoginConfig {
            oauth: None,
            session_secret: "session".into(),
            db_path: dir.join("accounts.db"),
            oauth_base_url: String::new(),
            api_base_url: String::new(),
        })
        .expect("a scratch accounts database opens");
        Self { dir, accounts }
    }
}

impl Drop for AccountsScratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn values(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// Exported credentials become the file a submission of them would have, so a
/// launch that has them is not asked again; anything short of the LiveKit three
/// is left for Setup to ask.
#[test]
fn exported_credentials_become_the_file_setup_would_write() {
    let livekit = [
        ("LIVEKIT_URL", " wss://example.livekit.cloud "),
        ("LIVEKIT_API_KEY", "key"),
        ("LIVEKIT_API_SECRET", "secret"),
    ];
    assert_eq!(
        super::config_from_values(&values(&livekit), false)
            .unwrap()
            .unwrap(),
        "LIVEKIT_URL=wss://example.livekit.cloud\nLIVEKIT_API_KEY=key\nLIVEKIT_API_SECRET=secret\n"
    );

    // The Google keys ride along when there are any, and nothing else does: the
    // whole file is compared, so no other exported key can slip in.
    let mut full = livekit.to_vec();
    full.extend([
        ("GOOGLE_API_KEY", "google"),
        ("GOOGLE_API_KEYS", "one,two"),
        ("NODE_ENV", "production"),
        ("CODETRIAL_DB_PATH", "/elsewhere.db"),
    ]);
    assert_eq!(
        super::config_from_values(&values(&full), false)
            .unwrap()
            .unwrap(),
        "LIVEKIT_URL=wss://example.livekit.cloud\nLIVEKIT_API_KEY=key\nLIVEKIT_API_SECRET=secret\n\
         GOOGLE_API_KEY=google\nGOOGLE_API_KEYS=one,two\n"
    );

    for missing in 0..livekit.len() {
        let mut partial = livekit.to_vec();
        partial[missing].1 = " ";
        assert_eq!(
            super::config_from_values(&values(&partial), false),
            Ok(None)
        );
    }
}

/// Refused rather than written, for the reasons a submission is: the file
/// cannot carry the value, or the launch reading it back would refuse the URL
/// or the Gemini key list.
#[test]
fn exported_credentials_the_file_cannot_hold_are_refused() {
    for (key, value) in [
        ("LIVEKIT_API_SECRET", "secret\nSESSION_SECRET=chosen"),
        ("LIVEKIT_API_KEY", "\"key\""),
        ("LIVEKIT_URL", "ftp://example.livekit.cloud"),
        ("GOOGLE_API_KEYS", "one\nSESSION_SECRET=chosen"),
        ("GOOGLE_API_KEYS", "one,"),
        ("GOOGLE_API_KEYS", "one,,two"),
        ("GOOGLE_API_KEYS", "one,one"),
    ] {
        let mut pairs = vec![
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "key"),
            ("LIVEKIT_API_SECRET", "secret"),
        ];
        pairs.retain(|(name, _)| *name != key);
        pairs.push((key, value));
        assert!(
            super::config_from_values(&values(&pairs), false).is_err(),
            "{key}={value:?}"
        );
    }
}

fn offer_file(label: &str, text: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "codetrial-offer-{label}-{}.env",
        std::process::id()
    ));
    std::fs::write(&path, text).unwrap();
    path
}

/// The rotation list outranks the single key, as it does when the server reads
/// the file, and the form takes one key, so the first is used and the page is
/// told how many there were.
#[test]
fn an_offered_rotation_list_gives_its_first_key_and_says_so() {
    let path = offer_file(
        "rotation",
        "LIVEKIT_URL=wss://example.livekit.cloud\nLIVEKIT_API_KEY=key\n\
         LIVEKIT_API_SECRET=secret\nGOOGLE_API_KEY=single\nGOOGLE_API_KEYS= first , second\n",
    );
    let (submission, endpoint, note) = read_offer(&path).unwrap();
    let _ = std::fs::remove_file(&path);

    assert_eq!(endpoint, "wss://example.livekit.cloud");
    assert_eq!(submission["googleApiKey"], "first");
    assert_eq!(submission["livekitApiSecret"], "secret");
    assert!(note.unwrap().starts_with("GOOGLE_API_KEYS lists 2 keys"));

    // A list of one is just that key, with nothing to say about the rest.
    let path = offer_file(
        "rotation-one",
        "LIVEKIT_URL=wss://example.livekit.cloud\nLIVEKIT_API_KEY=key\n\
         LIVEKIT_API_SECRET=secret\nGOOGLE_API_KEYS=only\n",
    );
    let (submission, _, note) = read_offer(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(submission["googleApiKey"], "only");
    assert_eq!(note, None);
}

/// The reason names the file once, whichever way it could not be used, and
/// names a malformed line by its number: the reason reaches the page, and the
/// line can be a key with its `=` missing.
#[test]
fn an_unusable_offer_says_why_without_repeating_the_file() {
    let missing = std::env::temp_dir().join("codetrial-offer-missing-nowhere.env");
    let error = read_offer(&missing).unwrap_err();
    assert_eq!(
        error.matches("codetrial-offer-missing-nowhere.env").count(),
        1,
        "{error}"
    );

    let broken = offer_file(
        "broken",
        "# saved by hand\nLIVEKIT_URL=wss://example.livekit.cloud\nLIVEKIT_API_SECRET the-secret\n",
    );
    let error = read_offer(&broken).unwrap_err();
    let _ = std::fs::remove_file(&broken);
    assert_eq!(
        error,
        format!("{}: line 3 is not KEY=value", broken.display())
    );

    let unusable = offer_file(
        "bad-url",
        "LIVEKIT_URL=ftp://example.livekit.cloud/?token=the-token\nLIVEKIT_API_KEY=key\n\
         LIVEKIT_API_SECRET=secret\n",
    );
    let error = read_offer(&unusable).unwrap_err();
    let _ = std::fs::remove_file(&unusable);
    assert!(error.contains("LIVEKIT_URL must start with"), "{error}");
    assert!(!error.contains("the-token"), "{error}");
}

/// Only the scheme and the host reach the page: user information, a path and
/// a query are where a LiveKit URL carries a credential.
#[test]
fn an_offered_url_is_shown_as_its_scheme_and_host() {
    let path = offer_file(
        "credentialed-url",
        "LIVEKIT_URL=wss://user:the-password@example.livekit.cloud:7880/prefix?token=the-token\n\
         LIVEKIT_API_KEY=key\nLIVEKIT_API_SECRET=secret\n",
    );
    let (_, endpoint, _) = read_offer(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(endpoint, "wss://example.livekit.cloud:7880");
}

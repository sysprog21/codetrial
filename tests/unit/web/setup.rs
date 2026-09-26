//! The `tests` module of `src/web/setup.rs`, which declares this file by path.
//! Everything here reaches into `src/web/setup.rs` through `super`, so it is a
//! unit
//! test and not an integration test: private items are in scope.

use super::host_is_loopback;
use axum::http::HeaderValue;

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

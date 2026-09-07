//! The `migration_tests` module of `src/accounts/schema.rs`, which declares
//! this file by path.
//! Everything here reaches into `src/accounts/schema.rs` through `super`, so it
//! is a unit
//! test and not an integration test: private items are in scope.

use super::*;
use crate::accounts::*;

/// Unverified sign-ins used to accumulate one account row and one session
/// row each, forever. Rate limiting bounds the rate, not the total.
#[test]
fn sweeping_removes_expired_throwaways_and_keeps_everything_earned() {
    // Its own directory because the sweep asserts on what is left on disk, and
    // a `Scratch` guard so the directory goes when the test does.
    let dir = Scratch(std::env::temp_dir().join(format!(
        "codetrial-sweep-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )));
    std::fs::create_dir_all(&*dir).unwrap();
    let path = dir.join("accounts.db");
    initialize_account_database(&path).unwrap();
    let accounts = Accounts::open(GitHubLoginConfig {
        oauth: None,
        session_secret: "session".into(),
        db_path: path.clone(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();

    accounts
        .with(|connection| {
            // An expired throwaway, a live throwaway, an expired throwaway that
            // owns a report, and an expired real GitHub account.
            connection.execute_batch(
                "
                INSERT INTO users (id, github_id, login, created_at, updated_at)
                  VALUES (-1, -1, 'gone', 1, 1);
                INSERT INTO users (id, github_id, login, created_at, updated_at)
                  VALUES (-2, -2, 'live', 1, 1);
                INSERT INTO users (id, github_id, login, created_at, updated_at)
                  VALUES (-3, -3, 'authored', 1, 1);
                INSERT INTO users (id, github_id, login, created_at, updated_at)
                  VALUES (7, 7, 'real', 1, 1);
                INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES ('a', -1, 100, 1);
                INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES ('b', -2, 9999999, 1);
                INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES ('c', -3, 100, 1);
                INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES ('d', 7, 100, 1);
                INSERT INTO reports (id, user_id, problem_id, payload, created_at, updated_at)
                  VALUES ('r', -3, 'two-sum', '{}', 1, 1);
                -- A recording is not reachable for a negative id through
                -- any route, because recording needs a verified address.
                -- Written directly, because the sweep must not depend on
                -- that rule holding two modules away: deleting this account
                -- cascades into `interviews`, and `recordings` references
                -- that with RESTRICT, so the row would fail the delete
                -- rather than be skipped by it.
                INSERT INTO users (id, github_id, login, created_at, updated_at)
                  VALUES (-4, -4, 'recorded', 1, 1);
                INSERT INTO sessions (id, user_id, expires_at, created_at) VALUES ('e', -4, 100, 1);
                INSERT INTO interviews (id, account_id, consent_version, consent_at, room_name)
                  VALUES ('int-sweep', -4, '2026-08-21', 1, 'interview-sweep01');
                INSERT INTO recordings (
                    id, account_id, interview_id, room_name, idempotency_key,
                    recipient_email, state, created_at, updated_at
                ) VALUES ('rec-sweep', -4, 'int-sweep', 'interview-sweep01', 'key-sweep',
                    'four@example.test', 'ready', 1, 1);
                ",
            )?;
            Ok(())
        })
        .unwrap();

    let (sessions, users) = sweep_expired_sessions(&accounts, 1000).unwrap();
    assert_eq!(sessions, 4, "four sessions were past their expiry");
    assert_eq!(users, 1, "only the throwaway that earned nothing goes");

    let remaining: Vec<i64> = accounts
        .with(|connection| {
            let mut statement = connection.prepare("SELECT id FROM users ORDER BY id")?;
            let rows = statement
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<i64>>>()?;
            Ok(rows)
        })
        .unwrap();

    // -1 swept. -2 kept, its session is live. -3 kept, it owns a report. -4
    // kept, it owns a recording, and sweeping it would have failed the
    // statement rather than skipped the row. 7 kept, a real GitHub account
    // outlives its sessions.
    assert_eq!(remaining, vec![-4, -3, -2, 7]);

    let _ = std::fs::remove_file(&path);
}

/// A scratch database that removes itself, WAL sidecars included, when the
/// test ends. Drop rather than a call at the end of each test, because a
/// test that ends by panicking is exactly the one whose leftovers you want
/// gone.
///
/// The path used to be the label alone. That bounded the mess, since each
/// run overwrote the last, but it also let two concurrent `cargo test` runs
/// migrate each other's database. Making it unique without also cleaning up
/// traded a rare race for an unbounded pile in the system temp directory.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        // Both shapes, because one test wants a directory to assert about what
        // the sweep left on disk and the rest want a single database. A scratch
        // path is whatever the test made of it.
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

fn scratch(label: &str) -> Scratch {
    Scratch(std::env::temp_dir().join(format!(
        "codetrial-migration-{label}-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )))
}

fn user_version(path: &Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn set_user_version(path: &Path, version: i64) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute_batch(&format!("PRAGMA user_version = {version}"))
        .unwrap();
}

/// Puts a database back into the shape it had at `version`, rather than
/// only rewinding the counter.
///
/// Rewinding alone was enough while every migration was idempotent, and
/// stopped being enough the moment one added a column: replaying
/// `ALTER TABLE ... ADD COLUMN` against a table that already has it fails
/// with `duplicate column name`. That failure is the fixture's, not the
/// migration's, so the fixture is what has to build an honest old database.
///
/// It was raised as a production idempotency flaw and is not one. `migrate`
/// runs only the entries after `PRAGMA user_version` and stamps the new
/// version inside the same `IMMEDIATE` transaction, so a migration runs
/// once or not at all; a database at version 3 holding version 4's columns
/// is a state only this helper can produce. Guarding the `ALTER` with a
/// `pragma_table_info` check would be defending against it.
///
/// `DROP COLUMN` needs SQLite 3.35, which the bundled build has and which
/// production never needs. That asymmetry is the price of the fixture and
/// is worth naming.
fn rewind_to(path: &Path, version: i64) {
    let connection = rusqlite::Connection::open(path).unwrap();

    // Newest first: `recordings` has foreign keys into `interviews`, so
    // dropping them the other way round would leave a table pointing at one
    // that is gone.
    if version < 10 {
        connection
            .execute_batch("DROP INDEX IF EXISTS recordings_by_room;")
            .unwrap();
    }
    if version < 9 {
        connection
            .execute_batch("DROP TABLE IF EXISTS delivery_queue;")
            .unwrap();
    }
    if version < 8 {
        connection
            .execute_batch(
                "DROP TABLE IF EXISTS replay_events;
                 ALTER TABLE recordings DROP COLUMN quota_exceeded;",
            )
            .unwrap();
    }
    if version < 7 {
        connection
            .execute_batch(
                "DROP TABLE IF EXISTS recording_events;
                 ALTER TABLE recordings DROP COLUMN stopped_at;",
            )
            .unwrap();
    }
    if version < 6 {
        connection
            .execute_batch(
                "DROP TABLE IF EXISTS recordings;
                 DROP INDEX IF EXISTS interviews_account_and_id;",
            )
            .unwrap();
    }
    if version < 5 {
        connection
            .execute_batch("DROP TABLE IF EXISTS interviews;")
            .unwrap();
    }
    if version < 4 {
        connection
            .execute_batch(
                "ALTER TABLE users DROP COLUMN email;
                 ALTER TABLE users DROP COLUMN email_verified;",
            )
            .unwrap();
    }
    connection
        .execute_batch(&format!("PRAGMA user_version = {version}"))
        .unwrap();
}

fn tables(path: &Path) -> Vec<String> {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Thirty days, as the constant's own comment claims, asserted rather than
/// assumed.
///
/// Every arithmetic mutation of `60 * 60 * 24 * 30` survived the suite:
/// nothing anywhere read the value, so `60 + 60 * 24 * 30` would have cut
/// every session to forty-four thousand seconds and `60 * 60 * 24 / 30` to
/// under an hour, and a candidate booking next week would be signed out
/// before they arrived. The expected side is a plain literal, because an
/// expression here would be mutated the same way the definition is and the
/// two would agree on the wrong number.
#[test]
fn the_session_lifetime_is_the_thirty_days_it_claims() {
    assert_eq!(SESSION_TTL_SECONDS, 2_592_000);
}

/// A GitHub id adopts the row it already owns. A self-declared one never
/// adopts a row at all.
///
/// The sign of the id is what picks the statement, and inverting that
/// comparison survived every test. It is the difference between a returning
/// candidate's second sign-in updating their row and failing outright, and
/// -- the half that matters -- between a repeated random negative id being
/// refused and being handed somebody else's reports, which is what
/// `create_session` says it must never do.
#[test]
fn only_a_github_id_adopts_the_row_it_already_owns() {
    let dir = scratch("adopt-existing");
    initialize_account_database(dir.as_ref()).unwrap();

    // Positive: GitHub vouched for it, so signing in again updates the row
    // rather than colliding with it.
    sign_in_as(dir.as_ref(), "octocat", 42);
    sign_in_as(dir.as_ref(), "octocat-renamed", 42);
    assert_eq!(logins(dir.as_ref()), vec!["octocat-renamed".to_string()]);

    // Negative: a self-declared handle. A second one carrying the same id has
    // to fail rather than take over the first one's row.
    sign_in_as(dir.as_ref(), "declared", -7);
    let collision = create_session(
        &accounts_at(dir.as_ref()),
        &GitHubProfile {
            github_id: -7,
            login: "impostor".to_string(),
            avatar_url: None,
            verified_email: None,
        },
    );
    assert!(
        collision.is_err(),
        "a repeated self-declared id must not adopt the row it collided with"
    );
    assert_eq!(
        logins(dir.as_ref()),
        vec!["declared".to_string(), "octocat-renamed".to_string()],
        "the collision must leave the row it hit exactly as it was"
    );
}

/// The stored expiry is the constant applied, not merely a number near it.
///
/// Pinning `SESSION_TTL_SECONDS` alone left the arithmetic that uses it
/// free: turning `now + SESSION_TTL_SECONDS` into a product survived,
/// because nothing read the row the session actually got.
#[test]
fn a_session_expires_one_lifetime_after_it_was_created() {
    let dir = scratch("session-expiry");
    initialize_account_database(dir.as_ref()).unwrap();
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    sign_in(dir.as_ref(), "clockwatcher");
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let expires: i64 = rusqlite::Connection::open(dir.as_ref())
        .unwrap()
        .query_row("SELECT expires_at FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert!(
        expires >= before + SESSION_TTL_SECONDS && expires <= after + SESSION_TTL_SECONDS,
        "expiry {expires} is not one lifetime after a creation between \
         {before} and {after}"
    );
}

/// Zero is the boundary of the sign test, and neither caller produces it.
///
/// `recorded_account_id` is negative by construction and GitHub ids are
/// positive, so nothing reaches this value in the product; the comparisons
/// still have to resolve it one way, and both of them resolve it to the
/// GitHub side. Asserted because the alternatives survive otherwise: a
/// zero id that adopted nothing would collide with itself on the second
/// sign-in, and one treated as vouched for would keep an address GitHub
/// never confirmed.
#[test]
fn a_zero_github_id_adopts_its_row_and_earns_no_verified_address() {
    let dir = scratch("zero-github-id");
    initialize_account_database(dir.as_ref()).unwrap();
    let profile = |login: &str| GitHubProfile {
        github_id: 0,
        login: login.to_string(),
        avatar_url: None,
        verified_email: Some("zero@example.test".to_string()),
    };
    create_session(&accounts_at(dir.as_ref()), &profile("zero")).unwrap();
    create_session(&accounts_at(dir.as_ref()), &profile("zero-renamed")).unwrap();
    assert_eq!(logins(dir.as_ref()), vec!["zero-renamed".to_string()]);

    let (email, verified): (Option<String>, i64) = rusqlite::Connection::open(dir.as_ref())
        .unwrap()
        .query_row("SELECT email, email_verified FROM users", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(email, None, "only a positive id is one GitHub vouched for");
    assert_eq!(verified, 0);
}

/// A report id somebody else holds is refused, not overwritten.
///
/// The ownership guard is the whole of that refusal, and dropping it
/// survived every test: the write below would have gone through and
/// replaced another account's report with this one's.
#[test]
fn a_report_id_another_account_holds_is_refused() {
    let dir = scratch("report-owner");
    initialize_account_database(dir.as_ref()).unwrap();
    let accounts = accounts_at(dir.as_ref());
    let holder = user_id_for(&accounts, &sign_in_as(dir.as_ref(), "holder", -1));
    let other = user_id_for(&accounts, &sign_in_as(dir.as_ref(), "other", -2));

    assert_eq!(
        save_report(
            &accounts,
            holder,
            "shared",
            "two-sum",
            &json!({"whose": "holder"})
        )
        .unwrap(),
        ReportSave::Saved
    );
    assert_eq!(
        save_report(
            &accounts,
            other,
            "shared",
            "two-sum",
            &json!({"whose": "other"})
        )
        .unwrap(),
        ReportSave::NotOwner
    );
    assert_eq!(list_reports(&accounts, other).unwrap().len(), 0);
    assert_eq!(
        list_reports(&accounts, holder).unwrap()[0]["payload"]["whose"],
        "holder",
        "the refused write must not have replaced the row it collided with"
    );
}

/// A room is claimed once, and the second claim says so.
///
/// The claim is an UPDATE guarded on the room still being unset, so its
/// row count is what separates taking the room from finding it taken.
/// Reading that count as anything but positive survived, which would have
/// handed two tokens the same interview.
#[test]
fn an_interview_room_is_claimed_once() {
    let dir = scratch("claim-room");
    initialize_account_database(dir.as_ref()).unwrap();
    let accounts = accounts_at(dir.as_ref());
    let account = user_id_for(&accounts, &sign_in(dir.as_ref(), "claimer"));
    create_interview(&accounts, "int-claim", account, "2026-08-21", 5).unwrap();

    assert!(claim_interview_room(&accounts, "int-claim", account, "interview-aaa").unwrap());
    assert!(
        !claim_interview_room(&accounts, "int-claim", account, "interview-bbb").unwrap(),
        "a room already claimed is not claimed again"
    );
}

fn accounts_at(path: &Path) -> Accounts {
    Accounts::open(GitHubLoginConfig {
        oauth: None,
        session_secret: "migration-test".to_string(),
        db_path: path.to_path_buf(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap()
}

fn sign_in(path: &Path, login: &str) -> String {
    sign_in_as(path, login, -1)
}

/// `github_id` is UNIQUE, and production mints a fresh negative id per
/// self-declared login. A test needing two distinct accounts has to do the
/// same or it collides on the constraint.
fn sign_in_as(path: &Path, login: &str, github_id: i64) -> String {
    create_session(
        &accounts_at(path),
        &GitHubProfile {
            // Negative: a self-declared handle, which must never adopt an
            // existing row.
            github_id,
            login: login.to_string(),
            avatar_url: None,
            verified_email: None,
        },
    )
    .unwrap()
}

/// Migration 3 empties `sessions`, and `open_accounts` runs the sweep
/// immediately behind it, so the two delete more together than either does
/// alone. What survives is the part worth pinning: a real GitHub account
/// keeps its row, and anything holding a report keeps its row whatever its
/// id, while the throwaway accounts whose only reason to exist was the
/// session that just went away are reclaimed.
#[test]
fn the_upgrade_and_the_sweep_together_spare_real_accounts_and_reports() {
    let path = scratch("upgrade-sweep");
    initialize_account_database(&path).unwrap();
    let real = sign_in_as(&path, "real-candidate", 4242);
    let throwaway = sign_in_as(&path, "unverified", -7);
    let author = sign_in_as(&path, "wrote-something", -8);
    let accounts = accounts_at(&path);
    save_report(
        &accounts,
        user_id_for(&accounts, &author),
        "kept-report",
        "two-sum",
        &json!({}),
    )
    .unwrap();
    // Rewind to a database the hashing migration has not reached.
    rewind_to(&path, 2);
    drop(accounts);

    initialize_account_database(&path).unwrap();
    let accounts = accounts_at(&path);
    let (swept_sessions, swept_users) =
        sweep_expired_sessions(&accounts, current_epoch_seconds() as i64).unwrap();

    // Migration 3 already emptied the table, so the sweep finds no expired
    // session of its own to remove.
    assert_eq!(swept_sessions, 0);
    assert_eq!(swept_users, 1, "only the throwaway with nothing to keep");
    assert_eq!(
        logins(&path),
        vec!["real-candidate".to_string(), "wrote-something".to_string()]
    );
    assert!(session_ids(&path).is_empty());
    for session in [&real, &throwaway, &author] {
        assert!(
            session_user(&accounts, session).unwrap().is_none(),
            "every pre-upgrade cookie stops working"
        );
    }
    assert_eq!(
        list_reports(&accounts, user_id_for_login(&path, "wrote-something"))
            .unwrap()
            .len(),
        1,
        "the report outlives the session that produced it"
    );
}

fn user_id_for_login(path: &Path, login: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("SELECT id FROM users WHERE login = ?1", [login], |row| {
            row.get(0)
        })
        .unwrap()
}

/// The column pair is what makes "verified" a thing the server can check
/// rather than a thing it assumes, and an existing deployment has to gain
/// it without losing anyone's account.
#[test]
fn an_existing_database_gains_the_verified_email_columns() {
    let path = scratch("verified-email");
    initialize_account_database(&path).unwrap();
    sign_in_as(&path, "early-candidate", 4242);
    rewind_to(&path, 3);
    assert!(!columns(&path, "users").contains(&"email".to_string()));

    initialize_account_database(&path).unwrap();

    assert_eq!(user_version(&path), ACCOUNT_SCHEMA_VERSION);
    let columns = columns(&path, "users");
    assert!(columns.contains(&"email".to_string()));
    assert!(columns.contains(&"email_verified".to_string()));
    assert_eq!(logins(&path), vec!["early-candidate".to_string()]);

    // Everyone who predates the migration is unverified, which is what they
    // were. Defaulting the flag the other way would hand a delivery target to
    // every account that already existed.
    let (email, verified): (Option<String>, i64) = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT email, email_verified FROM users WHERE github_id = 4242",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(email, None);
    assert_eq!(verified, 0);
}

/// An index that exists but that the planner declines to use buys nothing,
/// so this asserts the plan rather than the schema. `recording_for_room` is
/// on the webhook path, which LiveKit retries, and it scanned the whole
/// table before the index existed.
#[test]
fn an_existing_database_gains_the_room_lookup_index() {
    let path = scratch("room-index");
    initialize_account_database(&path).unwrap();
    rewind_to(&path, 9);
    assert!(!index_names(&path).contains(&"recordings_by_room".to_string()));

    initialize_account_database(&path).unwrap();

    assert_eq!(user_version(&path), ACCOUNT_SCHEMA_VERSION);
    let plan = query_plan(&path);
    assert!(
        plan.contains("USING INDEX recordings_by_room"),
        "the room lookup still does not use the index: {plan}"
    );
}

/// How SQLite says it will answer the query `recording_for_room` issues.
fn query_plan(path: &Path) -> String {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "EXPLAIN QUERY PLAN SELECT id FROM recordings WHERE room_name = ?1",
            ["interview-anything"],
            |row| row.get::<_, String>(3),
        )
        .unwrap()
}

fn columns(path: &Path, table: &str) -> Vec<String> {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The persistence boundary, exercised directly. `primary_verified_email`
/// trims its own output, but `create_session` is public and is the thing
/// that derives the flag, so the guard has to hold when it is called with
/// something the OAuth path would never produce.
#[test]
fn a_blank_address_is_not_a_verified_one() {
    let path = scratch("blank-address");
    initialize_account_database(&path).unwrap();
    let accounts = accounts_at(&path);
    create_session(
        &accounts,
        &GitHubProfile {
            github_id: 4242,
            login: "real-candidate".to_string(),
            avatar_url: None,
            verified_email: Some("   ".to_string()),
        },
    )
    .unwrap();

    let (email, verified): (Option<String>, i64) = accounts
        .with(|connection| {
            connection.query_row(
                "SELECT email, email_verified FROM users WHERE github_id = 4242",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .unwrap();
    assert_eq!(email, None);
    assert_eq!(verified, 0, "whitespace is not a delivery address");
}

/// The upgrade an existing deployment actually performs. A fresh database
/// runs every migration in one go and proves nothing about the one that
/// runs alone against rows that are already there.
#[test]
fn an_existing_database_gains_the_recordings_table_on_upgrade() {
    let path = scratch("recordings-upgrade");
    initialize_account_database(&path).unwrap();
    let accounts = accounts_at(&path);
    create_session(
        &accounts,
        &GitHubProfile {
            github_id: 4242,
            login: "real-candidate".to_string(),
            avatar_url: None,
            verified_email: Some("real@example.test".to_string()),
        },
    )
    .unwrap();
    let user_id = user_id_for_login(&path, "real-candidate");
    create_interview(&accounts, "int-upgrade", user_id, "2026-08-21", 5).unwrap();
    drop(accounts);
    rewind_to(&path, 5);
    assert!(!tables(&path).contains(&"recordings".to_string()));

    initialize_account_database(&path).unwrap();

    assert_eq!(user_version(&path), ACCOUNT_SCHEMA_VERSION);
    assert!(tables(&path).contains(&"recordings".to_string()));
    assert_eq!(logins(&path), vec!["real-candidate".to_string()]);

    // The composite foreign key needs a unique index on the parent, and the
    // parent table already existed when this migration ran.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    connection
        .execute(
            "
            INSERT INTO recordings (
                id, account_id, interview_id, room_name, idempotency_key,
                recipient_email, state, created_at, updated_at
            ) VALUES ('rec-upgrade', ?1, 'int-upgrade', 'interview-up000001', 'key-upgrade',
                'real@example.test', 'starting', 6, 6)
            ",
            [user_id],
        )
        .unwrap();
}

#[test]
fn a_fresh_database_runs_every_migration_and_records_the_version() {
    let path = scratch("fresh");

    initialize_account_database(&path).unwrap();

    assert_eq!(user_version(&path), ACCOUNT_SCHEMA_VERSION);
    assert_eq!(
        tables(&path),
        vec![
            "delivery_queue",
            "interviews",
            "recording_events",
            "recordings",
            "replay_events",
            "reports",
            "sessions",
            "users"
        ]
    );
}

/// The case that matters when a second migration is appended: startup runs
/// this on every boot, so it has to be a no-op against a database that is
/// already current, and it has to leave the rows alone.
#[test]
fn re_initialising_is_a_no_op_and_keeps_existing_rows() {
    let path = scratch("idempotent");
    initialize_account_database(&path).unwrap();
    let session = sign_in(&path, "returning-candidate");

    initialize_account_database(&path).unwrap();
    initialize_account_database(&path).unwrap();

    assert_eq!(user_version(&path), ACCOUNT_SCHEMA_VERSION);
    let user = session_user(&accounts_at(&path), &session).unwrap();
    assert_eq!(
        user.map(|user| user.login),
        Some("returning-candidate".to_string())
    );
}

/// A binary rolled back after a migration ran. Today's queries against
/// tomorrow's tables is the one situation where continuing is worse than
/// refusing to start.
#[test]
fn a_database_from_a_future_version_is_refused() {
    let path = scratch("future");
    initialize_account_database(&path).unwrap();
    let session = sign_in(&path, "someone");
    set_user_version(&path, ACCOUNT_SCHEMA_VERSION + 1);

    assert!(initialize_account_database(&path).is_err());

    // Refused, not damaged: the rows are still there for the correct binary.
    set_user_version(&path, ACCOUNT_SCHEMA_VERSION);
    assert!(
        session_user(&accounts_at(&path), &session)
            .unwrap()
            .is_some()
    );
}

/// The retry loop only helps if it can tell a busy database from a broken
/// one. Classify a lock as permanent and a contended startup answers 503
/// for its whole uptime; classify a refusal as transient and a rolled-back
/// binary spends three timeouts discovering it.
#[test]
fn only_lock_contention_is_treated_as_worth_retrying() {
    let path = scratch("locked");
    initialize_account_database(&path).unwrap();

    let holder = rusqlite::Connection::open(&path).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let blocked = rusqlite::Connection::open(&path).unwrap();
    blocked.execute_batch("PRAGMA busy_timeout = 0").unwrap();
    let error = blocked.execute_batch("BEGIN IMMEDIATE").unwrap_err();
    assert!(is_locked(&error), "{error} should be worth retrying");
    holder.execute_batch("ROLLBACK").unwrap();

    set_user_version(&path, ACCOUNT_SCHEMA_VERSION + 1);
    let mut refused = rusqlite::Connection::open(&path).unwrap();
    let error = migrate(&mut refused).unwrap_err();
    assert!(!is_locked(&error), "{error} should be fatal");
}

/// `applied as usize` on a negative counter would wrap to an enormous index
/// and panic inside the slice. Refusing names the problem instead.
#[test]
fn a_corrupt_schema_counter_is_refused_rather_than_panicking() {
    let path = scratch("corrupt");
    initialize_account_database(&path).unwrap();
    set_user_version(&path, -1);

    assert!(initialize_account_database(&path).is_err());
}

fn user_id_for(accounts: &Accounts, session: &str) -> i64 {
    session_user(accounts, session).unwrap().unwrap().id
}

fn index_names(path: &Path) -> Vec<String> {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn session_ids(path: &Path) -> Vec<String> {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection.prepare("SELECT id FROM sessions").unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The point of the whole exercise: a copy of the file must not be a list
/// of live cookies. Checked against the token that was handed out rather
/// than against a digest this test computes itself, so a `session_key` that
/// quietly became the identity function fails here instead of passing by
/// agreeing with itself.
#[test]
fn the_database_never_holds_the_cookie_it_handed_out() {
    let path = scratch("hashed");
    initialize_account_database(&path).unwrap();
    let session = sign_in(&path, "candidate");
    let stored = session_ids(&path);

    assert_eq!(stored.len(), 1);
    assert_ne!(stored[0], session, "the cookie value was written down");
    assert!(
        session_user(&accounts_at(&path), &session)
            .unwrap()
            .is_some(),
        "the cookie itself still authenticates"
    );
    assert!(
        session_user(&accounts_at(&path), &stored[0])
            .unwrap()
            .is_none(),
        "what is stored must not work as a cookie on its own"
    );
}

fn logins(path: &Path) -> Vec<String> {
    let connection = rusqlite::Connection::open(path).unwrap();
    let mut statement = connection
        .prepare("SELECT login FROM users ORDER BY login")
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The upgrade path, not just the fresh one: a database created before the
/// index migration existed has to gain it on the next boot and keep its
/// rows. `IF NOT EXISTS` alone would hide a migration that never ran.
///
/// The session is the one thing that must not survive. It was written
/// before session ids were hashed, so v3 discards it, and the account it
/// belonged to has to outlive it or the migration is throwing away more
/// than the tokens.
#[test]
fn an_existing_database_gains_the_report_index_on_upgrade() {
    let path = scratch("upgrade");
    initialize_account_database(&path).unwrap();
    let session = sign_in(&path, "early-candidate");
    assert!(user_id_for(&accounts_at(&path), &session) > 0);
    // Rewind to the shape a v1 binary left behind.
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("DROP INDEX IF EXISTS reports_by_user")
        .unwrap();
    drop(connection);
    rewind_to(&path, 1);
    assert!(!index_names(&path).contains(&"reports_by_user".to_string()));

    initialize_account_database(&path).unwrap();

    assert_eq!(user_version(&path), ACCOUNT_SCHEMA_VERSION);
    assert!(index_names(&path).contains(&"reports_by_user".to_string()));
    assert_eq!(logins(&path), vec!["early-candidate".to_string()]);
    assert!(
        session_user(&accounts_at(&path), &session)
            .unwrap()
            .is_none(),
        "a session id stored in the clear does not survive the upgrade"
    );
}

#[test]
fn a_full_account_refuses_new_reports_but_still_rewrites_its_own() {
    let path = scratch("quota");
    initialize_account_database(&path).unwrap();
    let accounts = accounts_at(&path);
    let user = user_id_for(&accounts, &sign_in(&path, "prolific"));
    for index in 0..MAX_REPORTS_PER_USER {
        assert_eq!(
            save_report(&accounts, user, &format!("r{index}"), "two-sum", &json!({})).unwrap(),
            ReportSave::Saved,
        );
    }

    assert_eq!(
        save_report(&accounts, user, "one-too-many", "two-sum", &json!({})).unwrap(),
        ReportSave::AtCapacity,
    );

    // The interview in progress must still be able to save its result, or the
    // ceiling would break the feature it exists to protect.
    assert_eq!(
        save_report(&accounts, user, "r0", "two-sum", &json!({ "v": 2 })).unwrap(),
        ReportSave::Saved,
    );
    assert_eq!(
        list_reports(&accounts, user).unwrap().len() as i64,
        MAX_REPORTS_PER_USER
    );
}

#[test]
fn deleting_reports_is_scoped_idempotent_and_reclaims_quota() {
    let path = scratch("delete-reports");
    initialize_account_database(&path).unwrap();
    let accounts = accounts_at(&path);
    let deleting = user_id_for(&accounts, &sign_in_as(&path, "deleting", -1));
    let other = user_id_for(&accounts, &sign_in_as(&path, "other", -2));
    for index in 0..MAX_REPORTS_PER_USER {
        save_report(
            &accounts,
            deleting,
            &format!("r{index}"),
            "two-sum",
            &json!({}),
        )
        .unwrap();
    }
    save_report(&accounts, other, "other-report", "two-sum", &json!({})).unwrap();

    assert_eq!(
        delete_reports(&accounts, deleting).unwrap() as i64,
        MAX_REPORTS_PER_USER
    );
    assert!(list_reports(&accounts, deleting).unwrap().is_empty());
    assert_eq!(list_reports(&accounts, other).unwrap().len(), 1);
    assert_eq!(delete_reports(&accounts, deleting).unwrap(), 0);
    assert_eq!(
        save_report(&accounts, deleting, "after-clear", "two-sum", &json!({})).unwrap(),
        ReportSave::Saved,
    );
}

/// Reports written inside one second still come back in one order.
///
/// Both timestamps are whole seconds, so a candidate who finishes two
/// interviews in the same second ties on either sort key, and without a
/// third the answer would be whatever the query plan felt like. Anything
/// reading the list as a chronology needs it to be the same answer twice.
#[test]
fn reports_saved_in_the_same_second_come_back_in_a_stable_order() {
    let path = scratch("report-order");
    initialize_account_database(&path).unwrap();
    let accounts = accounts_at(&path);
    let user = user_id_for(&accounts, &sign_in(&path, "quick"));
    for id in ["a", "b", "c"] {
        assert_eq!(
            save_report(&accounts, user, id, "two-sum", &json!({})).unwrap(),
            ReportSave::Saved,
        );
    }

    let ids = |accounts: &Accounts| -> Vec<String> {
        list_reports(accounts, user)
            .unwrap()
            .iter()
            .map(|report| report["id"].as_str().unwrap().to_string())
            .collect()
    };

    // The tie-break named, not merely "the same twice": SQLite happens to
    // return small tables in insertion order, so asserting only that two reads
    // agree passes just as well with no third sort key at all.
    assert_eq!(ids(&accounts), ["c", "b", "a"]);
    // And on a connection that planned the statement for itself.
    assert_eq!(ids(&accounts_at(&path)), ["c", "b", "a"]);
}

/// The quota is per account, so one candidate filling up must not lock out
/// anyone else, and it must not change who owns which id.
#[test]
fn the_quota_is_per_account_and_does_not_leak_ownership() {
    let path = scratch("quota-scope");
    initialize_account_database(&path).unwrap();
    let accounts = accounts_at(&path);
    let full = user_id_for(&accounts, &sign_in_as(&path, "full-account", -1));
    let fresh = user_id_for(&accounts, &sign_in_as(&path, "fresh-account", -2));
    for index in 0..MAX_REPORTS_PER_USER {
        save_report(&accounts, full, &format!("r{index}"), "two-sum", &json!({})).unwrap();
    }

    assert_eq!(
        save_report(&accounts, fresh, "mine", "two-sum", &json!({})).unwrap(),
        ReportSave::Saved,
    );
    // Someone else's id is refused as theirs, not absorbed into the quota.
    assert_eq!(
        save_report(&accounts, fresh, "r0", "two-sum", &json!({})).unwrap(),
        ReportSave::NotOwner,
    );
    assert_eq!(list_reports(&accounts, fresh).unwrap().len(), 1);

    // And the full account gets the same answer: a taken id is a conflict, not
    // a ceiling, or the 409 the caller needs would arrive as a 507.
    assert_eq!(
        save_report(&accounts, full, "mine", "two-sum", &json!({})).unwrap(),
        ReportSave::NotOwner,
    );
}

/// The expiry is enforced by the lookup, not by the sweep. The sweep runs at
/// startup and on a timer, so between two runs the only thing standing
/// between a stale cookie and a signed-in session is the `expires_at`
/// comparison inside the query. Drop it and every cookie this deployment
/// ever issued is immortal, including the ones on machines that were lost,
/// which is the reason the column exists at all.
///
/// The row is asserted to still be there afterwards, because a test that
/// only checked for `None` would pass just as well against a lookup with no
/// expiry guard if something had quietly deleted the session first.
#[test]
fn a_session_stops_working_once_its_expiry_has_passed() {
    let path = scratch("expired-session");
    initialize_account_database(&path).unwrap();
    let accounts = accounts_at(&path);
    let session = sign_in(&path, "late-candidate");
    assert!(
        session_user(&accounts, &session).unwrap().is_some(),
        "a fresh session authenticates, or the rest of this proves nothing"
    );

    // Backdated rather than waited out: the lifetime is thirty days.
    accounts
        .with(|connection| connection.execute("UPDATE sessions SET expires_at = 1", []))
        .unwrap();

    assert!(
        session_user(&accounts, &session).unwrap().is_none(),
        "an expired cookie must not sign anyone in"
    );
    assert_eq!(
        session_ids(&path).len(),
        1,
        "the row is still on disk, so it is the query that refused it"
    );
}

/// `email_verified` gates the column, and nothing but this filter reads it
/// on the way out. The flag can go to zero after the address was stored:
/// GitHub reports an address as unverified once its owner removes the
/// confirmation, and the next sign-in writes that back. An address the flag
/// no longer vouches for is a recording-delivery target nobody confirmed,
/// which is a report mailed to whoever holds that mailbox now.
///
/// Written to the column directly, because the OAuth path clears the
/// address and the flag together and the guard exists for the case where
/// they disagree.
#[test]
fn an_address_the_flag_no_longer_vouches_for_is_not_handed_back() {
    let path = scratch("unvouched-address");
    initialize_account_database(&path).unwrap();
    let accounts = accounts_at(&path);
    let session = create_session(
        &accounts,
        &GitHubProfile {
            // Positive, because only an id GitHub issued ever earns a stored
            // address.
            github_id: 4242,
            login: "verified-candidate".to_string(),
            avatar_url: None,
            verified_email: Some("candidate@example.test".to_string()),
        },
    )
    .unwrap();
    assert_eq!(
        session_user(&accounts, &session)
            .unwrap()
            .unwrap()
            .verified_email,
        Some("candidate@example.test".to_string()),
        "a vouched-for address is handed back, or the rest of this proves nothing"
    );

    accounts
        .with(|connection| connection.execute("UPDATE users SET email_verified = 0", []))
        .unwrap();

    assert_eq!(
        session_user(&accounts, &session)
            .unwrap()
            .unwrap()
            .verified_email,
        None,
        "the flag went to zero, so the address is not a delivery target"
    );
    let stored: Option<String> = accounts
        .with(|connection| connection.query_row("SELECT email FROM users", [], |row| row.get(0)))
        .unwrap();
    assert_eq!(
        stored,
        Some("candidate@example.test".to_string()),
        "the column is unchanged, so it is the flag that withheld it"
    );
}

/// One logout signs out one browser. The `WHERE id = ?1` is the whole of
/// that: widen it and every sign-out is a sign-out of the deployment,
/// dropping every candidate mid-interview and answering the next request
/// from anyone with a 401 they cannot explain.
///
/// Three sessions, because the two widenings fail differently and a
/// narrower fixture cannot tell them apart. Dropping the clause outright
/// empties the table; scoping it to the account instead of the session
/// signs the same person out of their other browser, which is the shape a
/// "log me out" that reads the row first would naturally take. The signing
/// out account holds two of the three so that both are refused here.
///
/// The second session needs a positive `github_id`: a self-declared login
/// mints a fresh negative id per sign-in and `github_id` is UNIQUE, so
/// signing in twice as one account is only reachable through the id GitHub
/// issued, which adopts the row it already owns.
#[test]
fn signing_one_account_out_leaves_every_other_signed_in() {
    let path = scratch("scoped-logout");
    initialize_account_database(&path).unwrap();
    let accounts = accounts_at(&path);
    let leaving = sign_in_as(&path, "leaving", 7);
    let other_browser = sign_in_as(&path, "leaving", 7);
    let staying = sign_in_as(&path, "staying", -2);
    assert_eq!(session_ids(&path).len(), 3);

    delete_session(&accounts, &leaving).unwrap();

    assert!(
        session_user(&accounts, &leaving).unwrap().is_none(),
        "the browser that signed out is signed out"
    );
    assert_eq!(
        session_user(&accounts, &other_browser)
            .unwrap()
            .map(|user| user.login),
        Some("leaving".to_string()),
        "the same account's other browser is still signed in"
    );
    assert_eq!(
        session_user(&accounts, &staying)
            .unwrap()
            .map(|user| user.login),
        Some("staying".to_string()),
        "nobody else was signed out"
    );
    assert_eq!(
        session_ids(&path).len(),
        2,
        "exactly one row went, so the delete was scoped rather than lucky"
    );
}

/// The interview id is the candidate's own and travels in a request they
/// control, so ownership is the only thing separating claiming your own
/// interview from binding somebody else's consent to a room you are about
/// to be handed a token for. `AND account_id = ?2` is that check, and it is
/// the whole of it: the other guards ask whether the interview is claimable,
/// not whose it is.
///
/// The refused claim is followed by the owner's, because a claim that
/// refused everyone would satisfy the first assertion on its own.
#[test]
fn a_room_claim_is_refused_to_an_account_that_does_not_hold_the_interview() {
    let path = scratch("claim-owner");
    initialize_account_database(&path).unwrap();
    let accounts = accounts_at(&path);
    let holder = user_id_for(&accounts, &sign_in_as(&path, "holder", -1));
    let stranger = user_id_for(&accounts, &sign_in_as(&path, "stranger", -2));
    create_interview(&accounts, "int-owned", holder, "2026-08-21", 5).unwrap();

    assert!(
        !claim_interview_room(&accounts, "int-owned", stranger, "interview-bbb").unwrap(),
        "an interview this account does not hold is not claimable by it"
    );
    assert_eq!(
        interview_for_account(&accounts, "int-owned", holder)
            .unwrap()
            .unwrap()
            .room_name,
        None,
        "the refused claim must not have bound a room either"
    );

    assert!(
        claim_interview_room(&accounts, "int-owned", holder, "interview-aaa").unwrap(),
        "the account that does hold it still claims it"
    );
}

/// The constant and the list are one fact written twice, and only the list
/// is append-only by construction. A migration appended without bumping the
/// constant reaches a fresh database, because `migrate` runs everything
/// from index zero, and never reaches an existing one, because that
/// database's `user_version` already equals the constant and `migrate`
/// returns before looking. The deployment then holds two different schemas
/// and neither the startup nor the queries say so; the first symptom is a
/// missing table on the machine that has been up longest.
///
/// Stated as the count rather than as a literal so it stays true as the
/// list grows: each entry takes the database from index `n` to `n + 1`, so
/// the version after all of them is exactly how many there are.
#[test]
fn the_schema_version_is_the_number_of_migrations_there_are() {
    assert_eq!(
        ACCOUNT_SCHEMA_VERSION,
        ACCOUNT_MIGRATIONS.len() as i64,
        "a migration was appended without bumping ACCOUNT_SCHEMA_VERSION"
    );
}

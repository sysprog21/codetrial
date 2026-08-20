//! Account storage and session lifecycle: the SQLite side of signing in, kept
//! apart from the HTTP handlers in [`crate::web`] that call it. Nothing here
//! knows about axum, so the schema and the queries can be read and tested
//! without a router in the way.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};

use crate::current_epoch_seconds;

/// Thirty days. Long enough that a candidate booking an interview next week is
/// still signed in when they arrive.
pub const SESSION_TTL_SECONDS: i64 = 60 * 60 * 24 * 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubLoginConfig {
    /// Absent unless an OAuth app is configured. The two credentials are only
    /// ever useful together, so they travel together and a handler that has
    /// this at all has both.
    pub oauth: Option<GitHubOauth>,
    pub session_secret: String,
    pub db_path: PathBuf,
    pub oauth_base_url: String,
    pub api_base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubOauth {
    pub client_id: String,
    pub client_secret: String,
}

/// The row in `users`, keyed by the local id every other table references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedInUser {
    pub id: i64,
    pub login: String,
    pub avatar_url: Option<String>,
}

/// What GitHub told us. Its `github_id` is not the local `users.id`, which is
/// why this is a separate type rather than a reused [`SignedInUser`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubProfile {
    pub github_id: i64,
    pub login: String,
    pub avatar_url: Option<String>,
}

/// The account database and the settings that reach it, resolved once at
/// startup. Both used to be rebuilt per request: `login_config` allocated five
/// strings to answer questions about immutable config, and every query opened a
/// fresh SQLite handle, paying a file open and a schema read before doing any
/// work. One connection also means `PRAGMA foreign_keys` is actually in force,
/// which per-request handles never had, so `ON DELETE CASCADE` now applies.
pub struct Accounts {
    config: GitHubLoginConfig,
    connection: Mutex<rusqlite::Connection>,
}

impl Accounts {
    pub fn open(config: GitHubLoginConfig) -> rusqlite::Result<Self> {
        let connection = rusqlite::Connection::open(&config.db_path)?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        Ok(Self {
            config,
            connection: Mutex::new(connection),
        })
    }

    pub fn config(&self) -> &GitHubLoginConfig {
        &self.config
    }

    /// Runs the migrations on the connection the server will keep using, so a
    /// router that was handed a database nobody initialized still works and no
    /// request pays for the schema check.
    pub fn initialize_schema(&self) -> rusqlite::Result<()> {
        let mut connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        initialize_connection(&mut connection)
    }

    /// Writes are rare and single-writer here, so one guarded connection beats
    /// a pool. Callers already run on the blocking pool, so holding the lock
    /// across the query parks no async worker.
    ///
    /// The ceiling, so the next person does not have to rediscover it: every
    /// query in the process queues behind every other. These are indexed
    /// point lookups on a small database, so they retire in tens of
    /// microseconds, and it stops being enough when concurrent interviews
    /// times queries per interview approaches that rate. A connection pool is
    /// the answer at that point, not a second mutex.
    fn with<T>(
        &self,
        task: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        let connection = self
            .connection
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        task(&connection)
    }
}

/// Bumped whenever a migration is added below. SQLite carries it in the file
/// header, so an existing database announces which migrations it has already
/// run instead of quietly keeping an old shape behind `IF NOT EXISTS`.
pub const ACCOUNT_SCHEMA_VERSION: i64 = 3;

/// Migrations in order, each one taking the database from index `n` to `n + 1`.
/// Append, never edit: an entry that has already run somewhere will not run
/// again.
const ACCOUNT_MIGRATIONS: &[&str] = &[
    CREATE_ACCOUNT_TABLES,
    INDEX_REPORTS_BY_USER,
    DISCARD_CLEARTEXT_SESSIONS,
];

pub fn initialize_account_database(path: &Path) -> rusqlite::Result<()> {
    let mut connection = rusqlite::Connection::open(path)?;
    initialize_connection(&mut connection)
}

fn initialize_connection(connection: &mut rusqlite::Connection) -> rusqlite::Result<()> {
    // WAL, because every login is a write and the default rollback journal
    // fsyncs per statement while blocking readers. The timeout goes first:
    // converting a fresh database to WAL needs an exclusive lock, and without a
    // timeout already in force that conversion fails instantly the one time
    // another process is attached. Foreign keys belong here rather than inside
    // a migration, because SQLite ignores that pragma once a transaction is
    // open and a migration needing a cascade would find enforcement quietly
    // off.
    //
    // rusqlite already sets this exact timeout on every connection it opens, so
    // this line changes nothing today. It stays because the value is load
    // bearing for everything below and a library default is a thin place to
    // rest that on: the day rusqlite picks a different number, the failure
    // would be a startup race nobody could reproduce.
    connection.execute_batch("PRAGMA busy_timeout = 5000;")?;

    // Converting a fresh database to WAL takes an exclusive lock. Measured
    // against a held lock it waits out the full timeout and then fails anyway,
    // so it shares the retry boundary with the migration rather than getting
    // one shot at it. Both are idempotent and both fail the same way.
    //
    // No sleep between attempts: each one already spent up to five seconds
    // inside SQLite's busy handler, and a pause on top of that is rounding
    // error on a wait that long.
    //
    // The ceiling, stated because no test reaches it: this only matters when a
    // lock outlives a five second timeout, which needs a migration slower than
    // anything in ACCOUNT_MIGRATIONS today. Reaching it would mean sleeping
    // past that timeout in a unit test, so it is argued rather than proven. The
    // reason it is here at all is the asymmetry, since losing the race costs a
    // process-lifetime 503 while the guard costs eight lines.
    let mut attempts = 0;
    loop {
        attempts += 1;
        match connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .and_then(|()| migrate(connection))
        {
            Err(error) if attempts < MIGRATION_ATTEMPTS && is_locked(&error) => {}
            result => return result,
        }
    }
}

/// Three tries at five seconds each, which outlasts any migration this schema
/// is likely to grow and still fails inside a deployment's patience.
const MIGRATION_ATTEMPTS: u32 = 3;

fn is_locked(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

fn migrate(connection: &mut rusqlite::Connection) -> rusqlite::Result<()> {
    // Steady state is "already migrated", and every startup takes this path.
    // Reading first keeps it a plain read instead of a write transaction that
    // dirties the header page and fsyncs to store the version it already holds.
    // Out-of-range values fall through to be rejected below.
    let applied: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if applied == ACCOUNT_SCHEMA_VERSION {
        return Ok(());
    }

    // Take the write lock before re-reading user_version. Two processes
    // starting together must not both run the same migration batch, and the
    // read above is not part of this transaction, so it cannot be trusted.
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let applied: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if !(0..=ACCOUNT_SCHEMA_VERSION).contains(&applied) {
        // Above the range means running today's queries against tomorrow's
        // tables; below it means the counter is corrupt, and casting it to an
        // index would panic rather than say so.
        return Err(rusqlite::Error::InvalidQuery);
    }
    for migration in &ACCOUNT_MIGRATIONS[applied as usize..] {
        transaction.execute_batch(migration)?;
    }
    transaction.execute_batch(&format!("PRAGMA user_version = {ACCOUNT_SCHEMA_VERSION}"))?;
    transaction.commit()
}

/// Rows written before [`session_key`] existed hold the bearer token itself, so
/// there is nothing to migrate: the value that would have to be hashed is the
/// secret being hidden, and anyone who already copied the file has it. Everyone
/// signs in again once, which is also the correct response to tokens that were
/// stored in the clear.
const DISCARD_CLEARTEXT_SESSIONS: &str = "DELETE FROM sessions;";

const CREATE_ACCOUNT_TABLES: &str = "
        CREATE TABLE IF NOT EXISTS users (
            id INTEGER PRIMARY KEY,
            github_id INTEGER NOT NULL UNIQUE,
            login TEXT NOT NULL,
            avatar_url TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            expires_at INTEGER NOT NULL,
            created_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS reports (
            id TEXT PRIMARY KEY,
            user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            problem_id TEXT NOT NULL,
            payload TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
";

/// Every report read and the per-account quota below both filter on `user_id`,
/// which was a full scan of the table for want of one index.
const INDEX_REPORTS_BY_USER: &str = "
        CREATE INDEX IF NOT EXISTS reports_by_user ON reports(user_id);
";

/// A candidate accumulates one report per interview, so a few hundred is a
/// career. Without a ceiling `/api/reports` is an authenticated write with no
/// rate limit and no cap, and a client that picks its own row ids can grow the
/// database until the disk runs out.
pub const MAX_REPORTS_PER_USER: i64 = 200;

/// Three outcomes the caller has to tell apart: a bool could not distinguish
/// "that id is someone else's" from "you are out of room", and they need
/// different status codes.
#[derive(Debug, PartialEq, Eq)]
pub enum ReportSave {
    Saved,
    NotOwner,
    AtCapacity,
}

pub fn create_session(accounts: &Accounts, profile: &GitHubProfile) -> rusqlite::Result<String> {
    accounts.with(|connection| {
        let now = current_epoch_seconds() as i64;

        // A positive id came from GitHub and names a returning person, so it
        // updates the row it already owns. A negative one is a freshly minted
        // account for a self-declared handle and must never adopt an existing
        // row: if the random id ever repeats, the insert has to fail rather
        // than hand the new arrival somebody else's reports.
        const INSERT_USER: &str = "
        INSERT INTO users (github_id, login, avatar_url, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?4)
    ";
        const ADOPT_EXISTING: &str = "
        ON CONFLICT(github_id) DO UPDATE SET
            login = excluded.login,
            avatar_url = excluded.avatar_url,
            updated_at = excluded.updated_at
    ";
        let statement = if profile.github_id < 0 {
            INSERT_USER.to_string()
        } else {
            format!("{INSERT_USER}{ADOPT_EXISTING}")
        };
        connection.execute(
            &statement,
            (
                profile.github_id,
                profile.login.as_str(),
                profile.avatar_url.as_deref(),
                now,
            ),
        )?;
        let user_id = connection.query_row(
            "SELECT id FROM users WHERE github_id = ?1",
            [profile.github_id],
            |row| row.get::<_, i64>(0),
        )?;
        let session_id = random_token(32).map_err(|_| rusqlite::Error::InvalidQuery)?;
        connection.execute(
            "
        INSERT INTO sessions (id, user_id, expires_at, created_at)
        VALUES (?1, ?2, ?3, ?4)
        ",
            (
                session_key(&session_id),
                user_id,
                now + SESSION_TTL_SECONDS,
                now,
            ),
        )?;
        Ok(session_id)
    })
}

pub fn normalized_github_login(value: &str) -> String {
    value.trim().trim_start_matches('@').to_ascii_lowercase()
}

pub fn valid_github_login(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 39
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

/// A typed handle is a label, not proof of anything, so it must never be the
/// key an account is found by: deriving the id from the login would mean typing
/// someone else's handle lands on their row and hands over their reports. Each
/// recorded login therefore gets a fresh id, and a negative one, because GitHub
/// ids are positive and the two kinds of account must never collide.
pub fn recorded_account_id() -> Result<i64, getrandom::Error> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes)?;

    // Lands in `i64::MIN ..= -1` and cannot overflow, which negating a random
    // magnitude can. `github_id` is UNIQUE, so the one-in-2^63 repeat fails the
    // insert rather than quietly joining two people to one account.
    Ok(i64::MIN + (i64::from_be_bytes(bytes) & i64::MAX))
}

/// rusqlite is synchronous, so every account query runs on the blocking pool
/// instead of parking an async worker on disk I/O.
pub async fn blocking<T, F>(task: F) -> rusqlite::Result<T>
where
    F: FnOnce() -> rusqlite::Result<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .unwrap_or_else(|error| {
            // Callers only need "the query did not happen", but the reason a
            // blocking task died is not a SQL problem and must not vanish into
            // one.
            eprintln!("account database task did not complete: {error}");
            Err(rusqlite::Error::InvalidQuery)
        })
}

pub fn session_user(
    accounts: &Accounts,
    session_id: &str,
) -> rusqlite::Result<Option<SignedInUser>> {
    accounts.with(|connection| {
        let now = current_epoch_seconds() as i64;
        let mut statement = connection.prepare(
            "
        SELECT users.id, users.login, users.avatar_url
        FROM sessions
        JOIN users ON users.id = sessions.user_id
        WHERE sessions.id = ?1 AND sessions.expires_at > ?2
        ",
        )?;
        let mut rows = statement.query((session_key(session_id), now))?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(SignedInUser {
            id: row.get(0)?,
            login: row.get(1)?,
            avatar_url: row.get(2)?,
        }))
    })
}

pub fn delete_session(accounts: &Accounts, session_id: &str) -> rusqlite::Result<()> {
    accounts.with(|connection| {
        connection.execute(
            "DELETE FROM sessions WHERE id = ?1",
            [session_key(session_id)],
        )?;
        Ok(())
    })
}

/// Deletes expired sessions and the throwaway accounts left behind by them.
///
/// `POST /api/login` mints a fresh negative account id per submission, so every
/// unverified sign-in inserted one `users` row and one `sessions` row and
/// nothing ever removed either. Rate limiting bounded the rate, not the total,
/// so the database grew for the lifetime of the deployment.
///
/// Only negative ids are collected. A positive id is a real GitHub account and
/// must outlive its sessions, and any account still owning a report is kept
/// whatever its id, because the report is the thing worth keeping.
///
/// Returns the rows removed, so the caller can say so rather than sweeping
/// silently.
pub fn sweep_expired_sessions(accounts: &Accounts, now: i64) -> rusqlite::Result<(usize, usize)> {
    accounts.with(|connection| {
        let sessions = connection.execute("DELETE FROM sessions WHERE expires_at <= ?1", [now])?;
        let users = connection.execute(
            "
            DELETE FROM users
            WHERE github_id < 0
              AND id NOT IN (SELECT user_id FROM sessions)
              AND id NOT IN (SELECT user_id FROM reports)
            ",
            [],
        )?;
        Ok((sessions, users))
    })
}

pub fn list_reports(accounts: &Accounts, user_id: i64) -> rusqlite::Result<Vec<Value>> {
    accounts.with(|connection| {
        let mut statement = connection.prepare(
            "
        SELECT id, problem_id, payload, created_at, updated_at
        FROM reports
        WHERE user_id = ?1
        ORDER BY updated_at DESC, created_at DESC
        ",
        )?;
        statement
            .query_map([user_id], |row| {
                let payload: String = row.get(2)?;
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "problemId": row.get::<_, String>(1)?,
                    "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
                    "createdAt": row.get::<_, i64>(3)?,
                    "updatedAt": row.get::<_, i64>(4)?,
                }))
            })?
            .collect()
    })
}

pub fn save_report(
    accounts: &Accounts,
    user_id: i64,
    id: &str,
    problem_id: &str,
    payload: &Value,
) -> rusqlite::Result<ReportSave> {
    accounts.with(|connection| {
        let now = current_epoch_seconds() as i64;

        // Who holds the id, and how full the account is, in one round trip.
        // Counting only the rows matching both columns could not tell "that id
        // is someone else's" from "that id is new", so a full account was told
        // it was out of room when the real answer was that the id was taken.
        let (owner, total): (Option<i64>, i64) = connection.query_row(
            "
        SELECT
            (SELECT user_id FROM reports WHERE id = ?1),
            (SELECT COUNT(*) FROM reports WHERE user_id = ?2)
        ",
            (id, user_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        match owner {
            Some(existing) if existing != user_id => return Ok(ReportSave::NotOwner),

            // Only a new row counts against the quota. Rewriting a report the
            // candidate already owns has to keep working at the ceiling, or
            // finishing an interview would fail once the account filled up.
            Some(_) => {}
            None if total >= MAX_REPORTS_PER_USER => return Ok(ReportSave::AtCapacity),
            None => {}
        }
        let changed = connection.execute(
            "
        INSERT INTO reports (id, user_id, problem_id, payload, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?5)
        ON CONFLICT(id) DO UPDATE SET
            problem_id = excluded.problem_id,
            payload = excluded.payload,
            updated_at = excluded.updated_at
        WHERE reports.user_id = excluded.user_id
        ",
            (id, user_id, problem_id, payload.to_string().as_str(), now),
        )?;
        Ok(if changed > 0 {
            ReportSave::Saved
        } else {
            ReportSave::NotOwner
        })
    })
}

/// What the `sessions` table stores. The cookie carries the token itself and
/// only its digest is written down, so a copy of the database is a list of
/// useless strings rather than thirty days of live logins.
///
/// Unsalted on purpose: the input is 32 bytes from the OS, so there is no
/// dictionary to precompute, and a per-row salt would cost the lookup the
/// primary-key index it currently rides on.
///
/// Public so integration fixtures that pin explicit user ids can still write
/// the row `create_session` would have written, rather than keeping a second
/// copy of this rule that drifts.
pub fn session_key(token: &str) -> String {
    use sha2::Digest;
    URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(token.as_bytes()))
}

pub fn random_token(bytes: usize) -> Result<String, getrandom::Error> {
    let mut token = vec![0; bytes];
    getrandom::fill(&mut token)?;
    Ok(URL_SAFE_NO_PAD.encode(token))
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    /// Unverified sign-ins used to accumulate one account row and one session
    /// row each, forever. Rate limiting bounds the rate, not the total.
    #[test]
    fn sweeping_removes_expired_throwaways_and_keeps_everything_earned() {
        // Its own directory because the sweep asserts on what is left on disk,
        // and a `Scratch` guard so the directory goes when the test does.
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
                // An expired throwaway, a live throwaway, an expired throwaway
                // that owns a report, and an expired real GitHub account.
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
                    ",
                )?;
                Ok(())
            })
            .unwrap();

        let (sessions, users) = sweep_expired_sessions(&accounts, 1000).unwrap();
        assert_eq!(sessions, 3, "three sessions were past their expiry");
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

        // -1 swept. -2 kept, its session is live. -3 kept, it owns a report. 7
        // kept, a real GitHub account outlives its sessions.
        assert_eq!(remaining, vec![-3, -2, 7]);

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
            // Both shapes, because one test wants a directory to assert about
            // what the sweep left on disk and the rest want a single database.
            // A scratch path is whatever the test made of it.
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
        set_user_version(&path, 2);
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

    #[test]
    fn a_fresh_database_runs_every_migration_and_records_the_version() {
        let path = scratch("fresh");

        initialize_account_database(&path).unwrap();

        assert_eq!(user_version(&path), ACCOUNT_SCHEMA_VERSION);
        assert_eq!(tables(&path), vec!["reports", "sessions", "users"]);
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

        // Refused, not damaged: the rows are still there for the correct
        // binary.
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
        set_user_version(&path, 1);
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

        // The interview in progress must still be able to save its result, or
        // the ceiling would break the feature it exists to protect.
        assert_eq!(
            save_report(&accounts, user, "r0", "two-sum", &json!({ "v": 2 })).unwrap(),
            ReportSave::Saved,
        );
        assert_eq!(
            list_reports(&accounts, user).unwrap().len() as i64,
            MAX_REPORTS_PER_USER
        );
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

        // And the full account gets the same answer: a taken id is a conflict,
        // not a ceiling, or the 409 the caller needs would arrive as a 507.
        assert_eq!(
            save_report(&accounts, full, "mine", "two-sum", &json!({})).unwrap(),
            ReportSave::NotOwner,
        );
    }
}

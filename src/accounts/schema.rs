//! The shape of the account database and the migrations that reach it.
//!
//! Split out of [`super`] because it is the half that is read rather than
//! called: seventeen SQL constants and the three functions that apply them, in
//! front of the queries that use them. `ACCOUNT_MIGRATIONS` is append-only, so
//! this file only ever grows at the bottom, and keeping it apart means that
//! growth stops pushing the query code further from the top of a file.

use std::path::Path;

/// Bumped whenever a migration is added below. SQLite carries it in the file
/// header, so an existing database announces which migrations it has already
/// run instead of quietly keeping an old shape behind `IF NOT EXISTS`.
pub const ACCOUNT_SCHEMA_VERSION: i64 = 10;

/// Migrations in order, each one taking the database from index `n` to `n + 1`.
/// Append, never edit: an entry that has already run somewhere will not run
/// again.
const ACCOUNT_MIGRATIONS: &[&str] = &[
    CREATE_ACCOUNT_TABLES,
    INDEX_REPORTS_BY_USER,
    DISCARD_CLEARTEXT_SESSIONS,
    ADD_VERIFIED_EMAIL,
    CREATE_INTERVIEWS,
    CREATE_RECORDINGS,
    CREATE_RECORDING_EVENTS,
    CREATE_REPLAY_EVENTS,
    CREATE_DELIVERY_QUEUE,
    INDEX_RECORDINGS_BY_ROOM,
];

pub fn initialize_account_database(path: &Path) -> rusqlite::Result<()> {
    let mut connection = rusqlite::Connection::open(path)?;
    initialize_connection(&mut connection)
}

pub(crate) fn initialize_connection(connection: &mut rusqlite::Connection) -> rusqlite::Result<()> {
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

/// Where a recording can be delivered, and whether anyone checked.
///
/// Two columns rather than one, because "no address" and "an address nobody
/// verified" have to be told apart: the second is what a self-declared sign-in
/// would produce if it were ever allowed to write one, and delivering a
/// recording to it would mean mailing an interview to whoever typed the
/// handle. `email_verified` defaults to 0, so every row that predates this
/// migration is unverified, which is what it was.
const ADD_VERIFIED_EMAIL: &str = "
        ALTER TABLE users ADD COLUMN email TEXT;
        ALTER TABLE users ADD COLUMN email_verified INTEGER NOT NULL DEFAULT 0;
";

/// One row per interview a candidate agreed to have recorded.
///
/// Consent is a fact with a time and a version, not a boolean: the disclosure
/// text changes, and "they agreed" is worthless without "to what, and when".
/// The row is written before anything is recorded, so an interview with no row
/// here is an interview no Egress call may be made for.
///
/// `consent_withdrawn_at` is nullable and set once. Withdrawal does not delete
/// the row, because the record that consent existed and was taken back is
/// exactly what a later deletion has to be justified by.
///
/// `room_name` is claimed once, by the token request. One consent is one
/// interview: without the claim the same id mints recorded rooms without limit,
/// and a candidate who agreed to be recorded once would have agreed to all of
/// them.
///
/// The partial unique index is what makes "one pending interview per account
/// and wording" true across processes rather than only inside one. The
/// in-process mutex serializes this server's own queries and says nothing about
/// a second `codetrial web` on the same database, which is a supported shape.
/// It is keyed on the wording too, so a version bump takes fresh consent
/// instead of being blocked by the row it replaces.
///
/// `ON DELETE CASCADE` follows `reports`: an account that goes takes its
/// interviews with it.
const CREATE_INTERVIEWS: &str = "
        CREATE TABLE IF NOT EXISTS interviews (
            id TEXT PRIMARY KEY,
            account_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            consent_version TEXT NOT NULL,
            consent_at INTEGER NOT NULL,
            consent_withdrawn_at INTEGER,
            room_name TEXT
        );

        CREATE INDEX IF NOT EXISTS interviews_by_account ON interviews(account_id);

        CREATE UNIQUE INDEX IF NOT EXISTS interviews_pending_per_account
            ON interviews(account_id, consent_version)
            WHERE room_name IS NULL AND consent_withdrawn_at IS NULL;
";

/// One row per recording, and the only place CodeTrial knows a media file
/// exists.
///
/// Nothing here holds media. What it holds is the handles: which room, which
/// Egress job, which staged object, which Drive file and permission, and who
/// the file was shared with. Losing a row does not lose a recording, it loses
/// the ability to find and delete one, which is why the foreign key is
/// `RESTRICT` rather than `CASCADE`. Deleting an account cascades into
/// `interviews`, that cascade reaches this `RESTRICT`, and the whole delete
/// fails; the file outlives the row and nothing else knows where it is.
///
/// The two `CHECK`s are one rule read in both directions: a recording that
/// exists must know which room it recorded and who it is for, and a recording
/// that has been deleted must have given up both, along with every locator for
/// bytes that are gone. `NOT NULL` could state the first half and not the
/// second, and a tombstone that quietly kept a candidate's address would be a
/// deletion that deleted the wrong thing. The ids stay: `egress_id` and the
/// tombstone fields are what a later audit is answered from.
///
/// Removing the row itself stays an explicit step. `RESTRICT` does not soften
/// once `deleted_at` is set, so whatever deletes accounts has to delete their
/// recordings first.
///
/// The foreign key is composite, `(account_id, interview_id)` into
/// `interviews(account_id, id)`, rather than two independent references. Two
/// references can disagree: each would be satisfied, and the row would attach
/// one account's recording to another account's interview, which is an
/// authorization bug the schema would have permitted. There is no separate
/// reference to `users`, because it would add nothing: an account cannot be
/// deleted without cascading through `interviews`, and that is where the
/// refusal happens.
///
/// `interview_id` is UNIQUE: one interview is one recording. `idempotency_key`
/// is UNIQUE for the same reason a webhook dedups on its event id, which is
/// that a retried start must not become a second Egress job.
///
/// `room_name` and `recipient_email` are nullable and guarded by a `CHECK`
/// instead. Task 11 clears them when it tombstones, so `NOT NULL` would make
/// the deletion write impossible and force a sentinel; the `CHECK` says the
/// real rule, which is that a recording that has not been deleted must have
/// both.
///
/// `egress_id` is null until the provider answers, so its uniqueness is a
/// partial index. A plain UNIQUE would be satisfied by any number of nulls in
/// SQLite, which is the right behaviour and the wrong place to rely on it.
///
/// `recipient_email` is a copy, not a lookup. It records the address the file
/// was actually shared with; the account's verified address can change
/// afterwards and the permission that was granted did not.
///
/// `id TEXT PRIMARY KEY NOT NULL` says the same thing twice on purpose. SQLite
/// permits NULL in any primary key that is not an INTEGER rowid alias, and
/// permits several of them, so `PRIMARY KEY` alone is not the constraint it
/// reads as. `interviews.id` and `reports.id` carry the same hole; nothing
/// inserts a NULL id, and rebuilding a table another table's foreign key
/// already points at costs more than a hole no code path reaches. New tables
/// spell it out.
const CREATE_RECORDINGS: &str = "
        CREATE UNIQUE INDEX IF NOT EXISTS interviews_account_and_id
            ON interviews(account_id, id);

        CREATE TABLE IF NOT EXISTS recordings (
            id TEXT PRIMARY KEY NOT NULL,
            account_id INTEGER NOT NULL,
            interview_id TEXT NOT NULL UNIQUE,
            room_name TEXT,
            idempotency_key TEXT NOT NULL UNIQUE,
            egress_id TEXT,
            gcs_object TEXT,
            drive_file_id TEXT,
            drive_permission_id TEXT,
            recipient_email TEXT,
            state TEXT NOT NULL,
            error TEXT,
            retries INTEGER NOT NULL DEFAULT 0,
            duration_seconds INTEGER,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            started_at INTEGER,
            ended_at INTEGER,
            ready_at INTEGER,
            expires_at INTEGER,
            deleted_at INTEGER,
            delete_error TEXT,
            deleted_by TEXT,
            FOREIGN KEY (account_id, interview_id)
                REFERENCES interviews(account_id, id) ON DELETE RESTRICT,
            CHECK (
                deleted_at IS NOT NULL
                OR (room_name IS NOT NULL AND recipient_email IS NOT NULL)
            ),
            CHECK (
                deleted_at IS NULL
                OR (
                    room_name IS NULL
                    AND recipient_email IS NULL
                    AND gcs_object IS NULL
                    AND drive_file_id IS NULL
                    AND drive_permission_id IS NULL
                )
            )
        );

        CREATE INDEX IF NOT EXISTS recordings_by_account ON recordings(account_id);

        CREATE UNIQUE INDEX IF NOT EXISTS recordings_by_egress
            ON recordings(egress_id) WHERE egress_id IS NOT NULL;

        CREATE INDEX IF NOT EXISTS recordings_by_expiry
            ON recordings(expires_at) WHERE expires_at IS NOT NULL AND deleted_at IS NULL;
";

/// `recording_for_room` is the webhook's first query, and it ran as a full scan
/// of `recordings` until this existed. LiveKit delivers at-least-once, so the
/// scan repeated per retry, and the table it scanned grows with retention
/// rather than with load.
///
/// Partial, like `recordings_by_egress`: the table's own CHECK forces
/// `room_name` to NULL once `deleted_at` is set, so restricting the index to
/// non-null rooms leaves out exactly the soft-deleted rows the lookup can never
/// match. `room_name = ?1` implies the index predicate, so SQLite still uses
/// it.
const INDEX_RECORDINGS_BY_ROOM: &str = "
        CREATE INDEX IF NOT EXISTS recordings_by_room
            ON recordings(room_name) WHERE room_name IS NOT NULL;
";

/// Every webhook LiveKit has delivered, by its own event id.
///
/// Delivery is at-least-once: a retry is a fresh, validly signed message with a
/// new `createdAt` and a new signature, so nothing about the message tells it
/// apart from the first attempt except the `id`. `recordings.egress_id` cannot
/// serve, because `egress_started`, `egress_updated` and `egress_ended` all
/// carry the same one.
///
/// No foreign key. An event can arrive for a recording this server has never
/// heard of, and refusing to write it down is how the same unknown event gets
/// processed forever.
///
/// `recordings.stopped_at` arrives with them, because a recording can be
/// finished on this side and still running on the provider's: a stop that
/// failed after the row moved leaves a job nobody is watching, and `failed` is
/// terminal so the state alone can never say so again. A row with an egress id
/// and no `stopped_at` is a job that still needs stopping, whatever its state.
///
/// The migration orders by `created_at` before `rowid`. Insertion order is not
/// lifecycle order once a restore or an import is in the picture, and the
/// promise being made is that the newest active recording is the one kept.
///
/// `applied_at` is the difference between "seen" and "dealt with". Claiming an
/// event and then failing to apply it would lose it: LiveKit's retry finds the
/// id already there and does nothing. A row with a null `applied_at` is an
/// attempt that did not finish, and redelivery is allowed to try again.
///
/// The partial unique index alongside it is the one-active-per-account rule,
/// moved out of a `SELECT COUNT(*)` and into the database. Counting first and
/// inserting second is serialized by the mutex around this process's one
/// connection and by nothing at all across two, and a second `codetrial web` on
/// the same database is a supported shape. A second concurrent Egress job is a
/// second bill and a second file nobody consented to.
///
/// The `UPDATE` before it exists because the previous schema allowed what the
/// index forbids. An upgrade that met two active recordings for one account
/// would fail to create the index and take startup with it, so the older ones
/// are failed as `superseded` first. Nothing in production has reached that
/// state; a migration that only works on databases nobody has is not a
/// migration.
const CREATE_RECORDING_EVENTS: &str = "
        CREATE TABLE IF NOT EXISTS recording_events (
            event_id TEXT PRIMARY KEY NOT NULL,
            received_at INTEGER NOT NULL,
            applied_at INTEGER
        );

        CREATE INDEX IF NOT EXISTS recording_events_by_receipt
            ON recording_events(received_at);

        ALTER TABLE recordings ADD COLUMN stopped_at INTEGER;

        UPDATE recordings
        SET state = 'failed', error = COALESCE(error, 'superseded')
        WHERE state IN ('starting', 'recording', 'finalizing', 'transferring')
          AND rowid <> (
            SELECT other.rowid FROM recordings other
            WHERE other.account_id = recordings.account_id
              AND other.state IN ('starting', 'recording', 'finalizing', 'transferring')
            ORDER BY other.created_at DESC, other.rowid DESC
            LIMIT 1
          );

        CREATE UNIQUE INDEX IF NOT EXISTS recordings_one_active_per_account
            ON recordings(account_id)
            WHERE state IN ('starting', 'recording', 'finalizing', 'transferring');

        CREATE INDEX IF NOT EXISTS recordings_active_by_update
            ON recordings(updated_at)
            WHERE state IN ('starting', 'recording', 'finalizing', 'transferring');
";

/// The interview, replayable without the video.
///
/// A recording is an MP4 and a transcript; a replay is what the candidate was
/// looking at while it happened. The two are delivered together and stored
/// apart, because one of them is media a provider owns and the other is small
/// enough to keep and cheap enough to serve.
///
/// `(interview_id, seq)` is the primary key, so no two events can claim one
/// position. It does not rule out a gap: only the server allocating the number
/// does that, and this is what makes the allocation's answer durable.
///
/// The `CHECK`s carry what the application would otherwise be the only place to
/// know. A `kind` nothing renders and a negative sequence are both rows this
/// table has no meaning for, whoever wrote them. `at` is the browser's clock
/// and `received_at` is this server's, because the two disagree and only one of
/// them is trustworthy.
///
/// `ON DELETE CASCADE`, unlike `recordings`. Nothing here is a handle to media
/// somewhere else, so losing these rows loses only what they say.
const CREATE_REPLAY_EVENTS: &str = "
        CREATE TABLE IF NOT EXISTS replay_events (
            interview_id TEXT NOT NULL REFERENCES interviews(id) ON DELETE CASCADE,
            seq INTEGER NOT NULL,
            kind TEXT NOT NULL,
            at INTEGER NOT NULL,
            payload TEXT NOT NULL,
            bytes INTEGER NOT NULL,
            received_at INTEGER NOT NULL,
            PRIMARY KEY (interview_id, seq),
            CHECK (seq >= 0),
            CHECK (at >= 0),
            CHECK (bytes >= 0),
            CHECK (kind IN ('transcript', 'editor', 'tests', 'stage', 'avatar', 'lifecycle'))
        );

        CREATE INDEX IF NOT EXISTS replay_events_by_interview_and_kind
            ON replay_events(interview_id, kind, seq);

        ALTER TABLE recordings ADD COLUMN quota_exceeded INTEGER NOT NULL DEFAULT 0;
";

/// The work outstanding between a finished recording and a delivered one.
///
/// One row per recording that still owes a delivery, and no row for one that
/// does not: success deletes the row, and giving up deletes it too, because
/// `recordings.state` and `recordings.error` are where the outcome lives. A
/// queue that kept its history would be a second, disagreeing answer to what
/// happened to a recording.
///
/// `claimed_at` and `claim` are the whole concurrency story. A worker claims a
/// row before it touches Drive, so two workers cannot both see no
/// `drive_file_id`, both upload, and leave the loser's file in the Shared Drive
/// with nothing naming it. A claim older than the stale window is reclaimable,
/// because the worker holding it may have died with the process, and `claim` is
/// what keeps the worker that was replaced from writing over its replacement:
/// every later write names the claim it was made under.
///
/// `ON DELETE CASCADE` from `recordings`: a recording that is gone owes no
/// delivery. That is the opposite of the `RESTRICT` on the account key, and for
/// the opposite reason, since nothing here is a handle to media.
const CREATE_DELIVERY_QUEUE: &str = "
        CREATE TABLE IF NOT EXISTS delivery_queue (
            recording_id TEXT PRIMARY KEY NOT NULL
                REFERENCES recordings(id) ON DELETE CASCADE,
            attempts INTEGER NOT NULL DEFAULT 0,
            run_after INTEGER NOT NULL,
            claimed_at INTEGER,
            claim TEXT,
            error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            CHECK (attempts >= 0),
            CHECK (run_after >= 0)
        );

        CREATE INDEX IF NOT EXISTS delivery_queue_due ON delivery_queue(run_after);
";

/// Every report read and the per-account quota below both filter on `user_id`,
/// which was a full scan of the table for want of one index.
const INDEX_REPORTS_BY_USER: &str = "
        CREATE INDEX IF NOT EXISTS reports_by_user ON reports(user_id);
";

#[cfg(test)]
mod migration_tests {
    use super::*;
    use crate::accounts::*;

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

        // Negative: a self-declared handle. A second one carrying the same id
        // has to fail rather than take over the first one's row.
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
        // were. Defaulting the flag the other way would hand a delivery target
        // to every account that already existed.
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
        // return small tables in insertion order, so asserting only that two
        // reads agree passes just as well with no third sort key at all.
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

        // And the full account gets the same answer: a taken id is a conflict,
        // not a ceiling, or the 409 the caller needs would arrive as a 507.
        assert_eq!(
            save_report(&accounts, full, "mine", "two-sum", &json!({})).unwrap(),
            ReportSave::NotOwner,
        );
    }
}

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
#[path = "../../tests/unit/accounts/schema.migration_tests.rs"]
mod migration_tests;

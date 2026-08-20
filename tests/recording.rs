//! The recordings table, checked against a real SQLite file rather than
//! against the `CREATE TABLE` string that produced it.
//!
//! Reading the schema back is the point. A test that asserted on the constant
//! would pass on a database the constant never reached, and the failure this
//! guards against is exactly that: a migration appended but not run, or run
//! against a database whose version counter says it already was.

use std::path::{Path, PathBuf};

use codetrial::accounts::{ACCOUNT_SCHEMA_VERSION, initialize_account_database};

/// A scratch database that removes itself, WAL sidecars included. The same
/// shape `src/accounts.rs` uses for its own migration tests, and for the same
/// reason: a test that ends by panicking is the one whose leftovers you want
/// gone.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

impl Scratch {
    fn new(label: &str) -> Self {
        Self(std::env::temp_dir().join(format!(
            "codetrial-recording-{label}-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn open(&self) -> rusqlite::Connection {
        let connection = rusqlite::Connection::open(&self.0).unwrap();

        // Off by default on every new handle, and the cascade and restrict
        // behaviour below is invisible without it.
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        connection
    }
}

/// `(name, type, not null, default, primary key)` for one table, straight out
/// of SQLite.
fn columns(connection: &rusqlite::Connection, table: &str) -> Vec<(String, String, bool, bool)> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT name, type, \"notnull\", pk FROM pragma_table_info('{table}')"
        ))
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? != 0,
                row.get::<_, i64>(3)? != 0,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The `CREATE INDEX` SQLite stored. The text rather than the name alone,
/// because a partial index and a full one differ only in the tail of that
/// statement, and only the partial one is correct here.
fn index_sql(connection: &rusqlite::Connection, name: &str) -> String {
    connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
            [name],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap_or_else(|error| panic!("index {name} should exist: {error}"))
        .unwrap_or_else(|| panic!("index {name} should be one this schema wrote"))
}

/// An account, an interview, and the row a recording hangs off. Written
/// directly rather than through the HTTP routes, because this file is about the
/// schema and a router in the way would only be able to reach the states the
/// routes already allow.
fn seed(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "
            INSERT INTO users (id, github_id, login, email, email_verified, created_at, updated_at)
              VALUES (1, 101, 'one', 'one@example.test', 1, 1, 1);
            INSERT INTO interviews (id, account_id, consent_version, consent_at, room_name)
              VALUES ('int-1', 1, '2026-08-21', 10, 'interview-abc12345');
            ",
        )
        .unwrap();
}

fn insert_recording(
    connection: &rusqlite::Connection,
    id: &str,
    interview: &str,
    key: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        )
        VALUES (?1, 1, ?2, 'interview-abc12345', ?3, 'one@example.test', 'starting', 20, 20)
        ",
        (id, interview, key),
    )
}

#[test]
fn recording_schema_columns() {
    let scratch = Scratch::new("columns");
    initialize_account_database(scratch.path()).unwrap();
    let connection = scratch.open();

    let columns = columns(&connection, "recordings");
    let by_name = |name: &str| {
        columns
            .iter()
            .find(|(column, ..)| column == name)
            .unwrap_or_else(|| panic!("recordings should have a {name} column"))
            .clone()
    };

    // Required, because a row without one of these describes a recording
    // nothing can find, charge to anyone, or deliver.
    for name in [
        "id",
        "account_id",
        "interview_id",
        "idempotency_key",
        "state",
        "retries",
        "created_at",
        "updated_at",
    ] {
        assert!(by_name(name).2, "{name} must be NOT NULL");
    }

    // Required until the recording is deleted, and clearable after. `NOT NULL`
    // would make task 11's tombstone write impossible and force a sentinel, so
    // the rule is a CHECK instead: see `recording_schema_constraints`.
    for name in ["room_name", "recipient_email"] {
        assert!(
            !by_name(name).2,
            "{name} has to be clearable when the media is deleted"
        );
    }

    // Nullable, because each is a fact that is not known yet: the provider has
    // not answered, the file has not been staged, the transfer has not run, or
    // the recording has not ended.
    for name in [
        "egress_id",
        "gcs_object",
        "drive_file_id",
        "drive_permission_id",
        "error",
        "duration_seconds",
        "started_at",
        "ended_at",
        "ready_at",
        "expires_at",
        "deleted_at",
        "delete_error",
        "deleted_by",
    ] {
        assert!(
            !by_name(name).2,
            "{name} must be nullable, it is not known yet"
        );
    }

    // Both, because in SQLite they are different claims: a primary key that is
    // not an INTEGER rowid alias accepts NULL, and accepts more than one of
    // them. `PRIMARY KEY` alone does not say what it looks like it says.
    assert!(by_name("id").3, "the recording id is the primary key");
    assert!(
        by_name("id").2,
        "and NOT NULL, which PRIMARY KEY does not imply here"
    );
    assert_eq!(by_name("account_id").1, "INTEGER");
    assert_eq!(by_name("recipient_email").1, "TEXT");
    assert_eq!(by_name("retries").1, "INTEGER");
    assert_eq!(by_name("deleted_at").1, "INTEGER");
    assert_eq!(by_name("delete_error").1, "TEXT");
    assert_eq!(by_name("deleted_by").1, "TEXT");
}

#[test]
fn recording_schema_constraints() {
    let scratch = Scratch::new("constraints");
    initialize_account_database(scratch.path()).unwrap();
    let connection = scratch.open();
    seed(&connection);

    insert_recording(&connection, "rec-1", "int-1", "key-1").unwrap();

    // One interview is one recording.
    connection
        .execute(

            // With a room, like every interview that reaches a recording. A
            // pending one would trip the partial unique index that keeps a
            // reload from starting a second interview.
            "INSERT INTO interviews (id, account_id, consent_version, consent_at, room_name) VALUES ('int-2', 1, '2026-08-21', 11, 'interview-def67890')",
            [],
        )
        .unwrap();
    let duplicate_interview = insert_recording(&connection, "rec-2", "int-1", "key-2");
    assert!(
        duplicate_interview.is_err(),
        "a second recording for one interview must be refused"
    );

    // A retried start must not become a second Egress job.
    let duplicate_key = insert_recording(&connection, "rec-3", "int-2", "key-1");
    assert!(
        duplicate_key.is_err(),
        "a repeated idempotency key must be refused"
    );

    // An interview that does not exist cannot be recorded.
    let orphan = insert_recording(&connection, "rec-4", "int-missing", "key-4");
    assert!(
        orphan.is_err(),
        "the interview foreign key must be enforced"
    );

    // Nor can somebody else's. Two independent references would each be
    // satisfied here, and the row would attach one account's recording to
    // another account's interview: an authorization bug the schema permitted.
    connection
        .execute_batch(
            "
            INSERT INTO users (id, github_id, login, email, email_verified, created_at, updated_at)
              VALUES (2, 202, 'two', 'two@example.test', 1, 1, 1);
            INSERT INTO interviews (id, account_id, consent_version, consent_at, room_name)
              VALUES ('int-other', 2, '2026-08-21', 13, 'interview-jkl56789');
            ",
        )
        .unwrap();
    assert!(
        insert_recording(&connection, "rec-cross", "int-other", "key-cross").is_err(),
        "account 1 must not record account 2's interview"
    );

    // The rule `NOT NULL` could not express: both are required while the
    // recording exists, and both are cleared when it is deleted, because after
    // deletion they describe nothing.
    assert!(
        connection
            .execute(
                "UPDATE recordings SET recipient_email = NULL WHERE id = 'rec-1'",
                []
            )
            .is_err(),
        "a live recording must know who it is for"
    );
    assert!(
        connection
            .execute(
                "UPDATE recordings SET room_name = NULL WHERE id = 'rec-1'",
                []
            )
            .is_err(),
        "and which room it recorded"
    );

    // And the other direction, which `NOT NULL` could not have said at all: a
    // tombstone that quietly kept a candidate's address, or a locator for bytes
    // that are gone, would be a deletion that deleted the wrong thing.
    connection
        .execute(
            "UPDATE recordings SET egress_id = 'EG_audit', gcs_object = 'codetrial/rec-1.mp4', drive_file_id = 'drive-1', drive_permission_id = 'perm-1' WHERE id = 'rec-1'",
            [],
        )
        .unwrap();
    for kept in [
        "room_name = 'interview-abc12345'",
        "recipient_email = 'one@example.test'",
        "gcs_object = 'codetrial/rec-1.mp4'",
        "drive_file_id = 'drive-1'",
        "drive_permission_id = 'perm-1'",
    ] {
        assert!(
            connection
                .execute(
                    &format!(
                        "UPDATE recordings SET deleted_at = 99, room_name = NULL, recipient_email = NULL, gcs_object = NULL, drive_file_id = NULL, drive_permission_id = NULL, {kept} WHERE id = 'rec-1'"
                    ),
                    [],
                )
                .is_err(),
            "a tombstone must not keep {kept}"
        );
    }
    assert!(
        connection
            .execute(
                "UPDATE recordings SET deleted_at = 99, room_name = NULL, recipient_email = NULL, gcs_object = NULL, drive_file_id = NULL, drive_permission_id = NULL WHERE id = 'rec-1'",
                [],
            )
            .is_ok(),
        "a tombstone keeps ids and gives up the rest"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT egress_id FROM recordings WHERE id = 'rec-1'",
                [],
                |row| row.get::<_, Option<String>>(0)
            )
            .unwrap()
            .as_deref(),
        Some("EG_audit"),
        "the provider job id stays; it is what a later audit is answered from"
    );

    // Deleting the row that says a file exists is how the file becomes
    // unreachable and undeletable, so it fails rather than cascading. The
    // account cascade reaches this through `interviews`, so both routes are
    // covered by the same refusal.
    assert!(
        connection
            .execute("DELETE FROM interviews WHERE id = 'int-1'", [])
            .is_err(),
        "an interview with a recording cannot be deleted out from under it"
    );
    assert!(
        connection
            .execute("DELETE FROM users WHERE id = 1", [])
            .is_err(),
        "and neither can the account"
    );

    // The uniqueness that has to be partial: `egress_id` is null until the
    // provider answers. The predicate is asserted, not just the name, because a
    // full unique index would carry the same name and the wrong behaviour.
    let egress_index = index_sql(&connection, "recordings_by_egress");
    assert!(egress_index.contains("UNIQUE"), "{egress_index}");
    assert!(
        egress_index.contains("WHERE egress_id IS NOT NULL"),
        "{egress_index}"
    );
    let expiry_index = index_sql(&connection, "recordings_by_expiry");
    assert!(
        expiry_index.contains("WHERE expires_at IS NOT NULL AND deleted_at IS NULL"),
        "the sweeper reads rows that have a deadline and are not yet gone: {expiry_index}"
    );

    connection
        .execute(
            "UPDATE recordings SET egress_id = 'EG_one' WHERE id = 'rec-1'",
            [],
        )
        .unwrap();
    insert_recording(&connection, "rec-5", "int-2", "key-5").unwrap();
    assert!(
        connection
            .execute(
                "UPDATE recordings SET egress_id = 'EG_one' WHERE id = 'rec-5'",
                [],
            )
            .is_err(),
        "two rows must not claim one Egress job"
    );

    // Two rows waiting on the provider at once. Setting one row's null back to
    // null proved nothing; this is the case the partial index has to permit.
    connection
        .execute(
            "INSERT INTO interviews (id, account_id, consent_version, consent_at, room_name) VALUES ('int-3', 1, '2026-08-21', 12, 'interview-ghi01234')",
            [],
        )
        .unwrap();
    insert_recording(&connection, "rec-6", "int-3", "key-6").unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM recordings WHERE egress_id IS NULL",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2,
        "any number of rows may be waiting for the provider to answer"
    );

    assert_eq!(
        connection
            .query_row(
                "SELECT retries FROM recordings WHERE id = 'rec-1'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0,
        "a fresh recording has not been retried"
    );
}

#[test]
fn recording_schema_reopen() {
    let scratch = Scratch::new("reopen");

    // A fresh database, then the same database again. Startup runs this on
    // every boot, so it has to be a no-op the second time and it has to leave
    // the rows alone.
    initialize_account_database(scratch.path()).unwrap();
    {
        let connection = scratch.open();
        seed(&connection);
        insert_recording(&connection, "rec-1", "int-1", "key-1").unwrap();
    }
    initialize_account_database(scratch.path()).unwrap();
    let connection = scratch.open();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM recordings", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1,
        "re-initialising must not touch the rows"
    );
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        ACCOUNT_SCHEMA_VERSION
    );

    // The task list asked for the opposite of what this codebase does: an older
    // binary does not open a migrated database, it refuses, because running
    // today's queries against tomorrow's tables is the failure that has no
    // symptom until it has a bad one. That refusal is already pinned by
    // `a_database_from_a_future_version_is_refused` in `src/accounts.rs`, so it
    // is not restated here; `TODO.md` records the correction.
}

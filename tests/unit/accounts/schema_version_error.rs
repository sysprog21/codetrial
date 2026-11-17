//! The `schema_version_error_tests` module of `src/accounts/schema.rs`, kept
//! apart because the migration tests sit at their file's line ceiling.

use super::*;

fn version(connection: &rusqlite::Connection) -> i64 {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

/// A database a newer build migrated must say so, not read as a SQL problem.
#[test]
fn a_database_from_a_newer_build_says_its_schema_is_newer() {
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    initialize_connection(&mut connection).unwrap();
    connection
        .execute_batch(&format!(
            "PRAGMA user_version = {}",
            ACCOUNT_SCHEMA_VERSION + 1
        ))
        .unwrap();

    let error = initialize_connection(&mut connection)
        .unwrap_err()
        .to_string();

    assert!(
        error.contains(&format!(
            "database schema version {} is newer than this build supports ({})",
            ACCOUNT_SCHEMA_VERSION + 1,
            ACCOUNT_SCHEMA_VERSION
        )),
        "{error}"
    );
    assert!(!error.contains("not read-only"), "{error}");
    assert_eq!(version(&connection), ACCOUNT_SCHEMA_VERSION + 1);
}

/// A negative counter is corruption, and must not be reported as newer.
#[test]
fn a_negative_schema_version_is_called_corrupt() {
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    initialize_connection(&mut connection).unwrap();
    connection
        .execute_batch("PRAGMA user_version = -1")
        .unwrap();

    let error = initialize_connection(&mut connection)
        .unwrap_err()
        .to_string();

    assert!(error.contains("is corrupt"), "{error}");
    assert!(!error.contains("newer"), "{error}");
}

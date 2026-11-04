use std::path::Path;

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
pub(super) fn rewind_to(path: &Path, version: i64) {
    let connection = rusqlite::Connection::open(path).unwrap();

    // Newest first: `recordings` has foreign keys into `interviews`, so
    // dropping them the other way round would leave a table pointing at one
    // that is gone.
    if version < 14 {
        connection.execute_batch("DROP TABLE peer_audio_records; DROP TABLE peer_transcripts; DROP TABLE peer_observations;").unwrap();
    }
    if version < 13 {
        connection.execute_batch("DROP TABLE peer_pair_requests; DROP TABLE peer_availability; DROP TABLE peer_profiles;").unwrap();
    }
    if version < 12 {
        connection
            .execute_batch(
                "DROP TABLE peer_feedback;
             DROP TABLE peer_snapshots;
             DROP TABLE peer_rounds;
             DROP TABLE peer_active_users;
             DROP TABLE peer_invites;
             DROP TABLE peer_members;
             DROP TABLE peer_problem_exposures;
             DROP TABLE peer_sessions;",
            )
            .unwrap();
    }
    if version < 11 {
        // Rebuilt under the list version 10 had, rows and all, so the upgrade
        // has something to carry across.
        connection
            .execute_batch(
                "CREATE TABLE replay_events_old AS SELECT * FROM replay_events;
                 DROP TABLE replay_events;
                 CREATE TABLE replay_events (
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
                 INSERT INTO replay_events SELECT * FROM replay_events_old;
                 DROP TABLE replay_events_old;
                 CREATE INDEX replay_events_by_interview_and_kind
                     ON replay_events(interview_id, kind, seq);",
            )
            .unwrap();
    }
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
    if version < 2 {
        connection
            .execute_batch("DROP INDEX IF EXISTS reports_by_user;")
            .unwrap();
    }
    connection
        .execute_batch(&format!("PRAGMA user_version = {version}"))
        .unwrap();
}

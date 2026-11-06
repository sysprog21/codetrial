use super::*;
use crate::accounts::*;

#[test]
fn peer_migration_preserves_version_eleven_accounts() {
    let accounts = Accounts::open(GitHubLoginConfig {
        oauth: None,
        session_secret: "migration".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    accounts.with(|connection| {
        for migration in &ACCOUNT_MIGRATIONS[..11] {
            connection.execute_batch(migration)?;
        }
        connection.execute_batch("PRAGMA user_version = 11; INSERT INTO users (id, github_id, login, created_at, updated_at) VALUES (306, 306, 'peer-owner', 1, 1);")
    }).unwrap();
    accounts.initialize_schema().unwrap();
    accounts
        .with(|connection| {
            for table in [
                "peer_sessions",
                "peer_members",
                "peer_invites",
                "peer_active_users",
                "peer_rounds",
                "peer_snapshots",
                "peer_feedback",
                "peer_problem_exposures",
                "peer_availability",
                "peer_pair_requests",
                "peer_profiles",
            ] {
                let count: i64 = connection.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )?;
                assert_eq!(count, 1, "missing {table}");
            }
            let login: String =
                connection
                    .query_row("SELECT login FROM users WHERE id=306", [], |row| row.get(0))?;
            assert_eq!(login, "peer-owner");
            Ok(())
        })
        .unwrap();
}

/// Removes the database and its sidecars when the test ends, panic or not.
struct Cleanup(std::path::PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

#[test]
fn a_fresh_database_runs_every_migration_and_records_the_version() {
    let path = std::env::temp_dir().join(format!(
        "codetrial-peer-fresh-{}-{}.db",
        std::process::id(),
        random_token(12).unwrap()
    ));
    let _cleanup = Cleanup(path.clone());
    initialize_account_database(&path).unwrap();
    {
        let connection = rusqlite::Connection::open(&path).unwrap();
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, ACCOUNT_SCHEMA_VERSION);
        let tables = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(
            tables,
            vec![
                "delivery_queue",
                "interviews",
                "peer_active_users",
                "peer_audio_records",
                "peer_availability",
                "peer_feedback",
                "peer_invites",
                "peer_members",
                "peer_observations",
                "peer_pair_requests",
                "peer_problem_exposures",
                "peer_profiles",
                "peer_rounds",
                "peer_sessions",
                "peer_snapshots",
                "peer_transcripts",
                "recording_events",
                "recordings",
                "replay_events",
                "reports",
                "sessions",
                "users",
            ]
        );
    }
}

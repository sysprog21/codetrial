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
pub(crate) struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.0.display()));
        }
    }
}

impl Scratch {
    pub(crate) fn new(label: &str) -> Self {
        Self(std::env::temp_dir().join(format!(
            "codetrial-recording-{label}-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn open(&self) -> rusqlite::Connection {
        let connection = rusqlite::Connection::open(&self.0).unwrap();

        // Foreign keys are off by default on every new handle, and the cascade
        // and restrict behaviour below is invisible without it. The timeout is
        // for the lifecycle tests, where this connection and the one inside
        // `Accounts` are both writing to the same file.
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
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
pub(crate) fn seed(connection: &rusqlite::Connection) {
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
        "stopped_at",
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

    // One account records one interview at a time, and it is a partial unique
    // index rather than a counted check: counting first and inserting second is
    // serialized inside one process and by nothing across two.
    assert!(
        insert_recording(&connection, "rec-second-active", "int-2", "key-second").is_err(),
        "a second active recording for one account must be refused"
    );
    connection
        .execute(
            "UPDATE recordings SET state = 'ready' WHERE id = 'rec-1'",
            [],
        )
        .unwrap();
    assert!(
        insert_recording(&connection, "rec-second-active", "int-2", "key-second").is_ok(),
        "and allowed once the first one is finished"
    );
    connection
        .execute("DELETE FROM recordings WHERE id = 'rec-second-active'", [])
        .unwrap();
    connection
        .execute(
            "UPDATE recordings SET state = 'starting' WHERE id = 'rec-1'",
            [],
        )
        .unwrap();

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
                "UPDATE recordings SET state = 'deleted', deleted_at = 99, room_name = NULL, recipient_email = NULL, gcs_object = NULL, drive_file_id = NULL, drive_permission_id = NULL WHERE id = 'rec-1'",
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
    // rec-1 is `deleted` by now, so the one-active slot is free.
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

    // Out of an active state first: one account records one interview at a
    // time, and rec-5 is holding that slot.
    connection
        .execute(
            "UPDATE recordings SET state = 'ready' WHERE id = 'rec-5'",
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

    // The obvious expectation is the opposite of what this codebase does: an
    // older binary does not open a migrated database, it refuses, because
    // running today's queries against tomorrow's tables is the failure that has
    // no symptom until it has a bad one. That refusal is already pinned by
    // `a_database_from_a_future_version_is_refused` in `src/accounts.rs`, so it
    // is not restated here.
}

pub(crate) mod lifecycle {
    use std::sync::{Arc, Mutex};

    use codetrial::accounts::{Accounts, GitHubLoginConfig};
    use codetrial::config::RecordingConfig;
    use codetrial::recording::{
        Clock, Recorder, Recording, RecordingProvider, RecordingState, StartEgress, StartRefusal,
        begin_recording, claim_webhook_event, recording_by_id, sweep_recordings, transition,
    };

    use super::{Scratch, seed};

    /// A clock a test can move. The retry schedule is minutes long and the
    /// stale sweep is a quarter of an hour, so a test that waited for either
    /// would be a test nobody runs.
    pub(crate) struct TestClock(Mutex<i64>);

    impl TestClock {
        fn new(now: i64) -> Arc<Self> {
            Arc::new(Self(Mutex::new(now)))
        }

        pub(crate) fn advance(&self, seconds: i64) {
            *self.0.lock().unwrap() += seconds;
        }
    }

    impl Clock for TestClock {
        fn now(&self) -> i64 {
            *self.0.lock().unwrap()
        }
    }

    /// A provider that records what it was asked and answers however the test
    /// needs. Every failure this pipeline has to survive is the provider's, and
    /// none can be produced against LiveKit on demand.
    #[derive(Default)]
    pub(crate) struct FakeProvider {
        started: Mutex<Vec<StartEgress>>,
        stopped: Mutex<Vec<String>>,
        pub(crate) refuse_start: Mutex<bool>,
        pub(crate) refuse_stop: Mutex<bool>,
        next_egress: Mutex<usize>,
        /// An Egress job the provider already has for the room, which is what a
        /// start whose id was never written down leaves behind.
        pub(crate) adopt: Mutex<Option<String>>,
    }

    impl FakeProvider {
        fn arc() -> Arc<Self> {
            Arc::new(Self::default())
        }

        pub(crate) fn starts(&self) -> usize {
            self.started.lock().unwrap().len()
        }

        pub(crate) fn stops(&self) -> Vec<String> {
            self.stopped.lock().unwrap().clone()
        }
    }

    impl RecordingProvider for FakeProvider {
        fn start<'a>(
            &'a self,
            request: &'a StartEgress,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
        {
            Box::pin(async move {
                self.started.lock().unwrap().push(request.clone());
                if *self.refuse_start.lock().unwrap() {
                    return Err("the provider is busy".to_string());
                }
                let mut next = self.next_egress.lock().unwrap();
                *next += 1;
                Ok(format!("EG_fake{next}"))
            })
        }

        fn stop<'a>(
            &'a self,
            egress_id: &'a str,
            room_name: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>
        {
            Box::pin(async move {
                self.stopped
                    .lock()
                    .unwrap()
                    .push(format!("{egress_id}@{room_name}"));
                if *self.refuse_stop.lock().unwrap() {
                    return Err("the provider would not stop".to_string());
                }
                Ok(())
            })
        }

        fn active_for_room<'a>(
            &'a self,
            _room_name: &'a str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Option<String>, String>> + Send + 'a>,
        > {
            Box::pin(async move { Ok(self.adopt.lock().unwrap().clone()) })
        }
    }

    fn recording_config() -> RecordingConfig {
        RecordingConfig {
            livekit: None,
            gcs_bucket: "codetrial-staging".to_string(),
            gcs_prefix: "codetrial".to_string(),
            drive_id: "0AKfixtureDriveId".to_string(),
            service_account_json: "{}".to_string(),
            max_minutes: 45,
            bitrate: 2000,
            kill_switch: false,
            template_base_url: "https://recording.codetrial.example".to_string(),
        }
    }

    /// A database with an account and two interviews that have claimed rooms,
    /// plus the pieces a recording runs against.
    pub(crate) fn harness(
        label: &str,
    ) -> (
        Scratch,
        Arc<Accounts>,
        Arc<FakeProvider>,
        Arc<TestClock>,
        Recorder,
    ) {
        let scratch = Scratch::new(label);
        codetrial::accounts::initialize_account_database(scratch.path()).unwrap();

        // Seeded on its own connection, not through `Accounts`, which keeps its
        // guarded connection to itself. WAL makes the two coexist.
        {
            let connection = scratch.open();
            seed(&connection);
            connection
                .execute_batch(
                    "INSERT INTO interviews (id, account_id, consent_version, consent_at, room_name)
                       VALUES ('int-2', 1, '2026-08-21', 11, 'interview-def67890');",
                )
                .unwrap();
        }
        let accounts = Arc::new(
            Accounts::open(GitHubLoginConfig {
                oauth: None,
                session_secret: "lifecycle".to_string(),
                db_path: scratch.path().to_path_buf(),
                oauth_base_url: String::new(),
                api_base_url: String::new(),
            })
            .unwrap(),
        );
        let provider = FakeProvider::arc();
        let clock = TestClock::new(1_000);
        let recorder = Recorder {
            provider: provider.clone(),
            clock: clock.clone(),

            // Retention is the one path that needs a delivery provider on the
            // recorder, and the tests that exercise it put their own fake here.
            // Everything else runs without one, which is also what a deployment
            // whose credentials failed looks like.
            delivery: None,
            config: recording_config(),
        };
        (scratch, accounts, provider, clock, recorder)
    }

    pub(crate) fn start(
        accounts: &Accounts,
        recorder: &Recorder,
        interview: &str,
        recording_id: &str,
    ) -> Result<Recording, StartRefusal> {
        started(accounts, recorder, interview, recording_id).map(|(recording, _)| recording)
    }

    /// The same start, keeping the answer to "did this call insert the row",
    /// which is the whole answer to "may this caller talk to the provider".
    fn started(
        accounts: &Accounts,
        recorder: &Recorder,
        interview: &str,
        recording_id: &str,
    ) -> Result<(Recording, bool), StartRefusal> {
        begin_recording(
            accounts,
            recorder.clock.as_ref(),
            interview,
            1,
            recording_id,
            "one@example.test",
            recorder.config.kill_switch,
        )
    }

    pub(crate) fn state_of(accounts: &Accounts, id: &str) -> RecordingState {
        recording_by_id(accounts, id).unwrap().unwrap().state
    }

    #[test]
    fn lifecycle_transitions_refuse_what_the_table_does_not_have() {
        use RecordingState::*;

        // Every state may become itself, because a duplicate webhook and a
        // second stop both ask for a transition that already happened.
        for state in [
            Starting,
            Recording,
            Finalizing,
            Transferring,
            Ready,
            Failed,
            CleanupFailed,
            Deleted,
        ] {
            assert!(
                state.may_become(state),
                "{} must be idempotent",
                state.as_str()
            );
        }

        assert!(Starting.may_become(Recording));
        assert!(Starting.may_become(Finalizing), "a stop can beat the start");
        assert!(
            Finalizing.may_become(Failed),
            "an end with no file is finished"
        );
        assert!(
            Failed.may_become(Deleted),
            "a failure can still have left bytes"
        );
        assert!(CleanupFailed.may_become(Deleted));

        assert!(!Ready.may_become(Recording), "nothing goes backwards");
        assert!(!Deleted.may_become(Ready), "deletion is terminal");
        assert!(!Transferring.may_become(Recording));
        assert!(!Starting.may_become(Ready), "no state may be skipped");
    }

    #[tokio::test]
    async fn lifecycle_one_active_per_account() {
        let (_scratch, accounts, _provider, _clock, recorder) = harness("one-active");

        start(&accounts, &recorder, "int-1", "rec-1").unwrap();
        assert_eq!(
            start(&accounts, &recorder, "int-2", "rec-2"),
            Err(StartRefusal::AlreadyActive),
            "an account records one interview at a time"
        );

        // Once the first is finished the second is allowed, or a candidate
        // could never be interviewed twice.
        transition(
            &accounts,
            recorder.clock.as_ref(),
            "rec-1",
            RecordingState::Finalizing,
            None,
        )
        .unwrap();
        transition(
            &accounts,
            recorder.clock.as_ref(),
            "rec-1",
            RecordingState::Failed,
            Some("abandoned"),
        )
        .unwrap();
        assert!(start(&accounts, &recorder, "int-2", "rec-2").is_ok());
    }

    #[tokio::test]
    async fn lifecycle_consent_withdrawn() {
        let (scratch, accounts, provider, _clock, recorder) = harness("withdrawn");
        let recording = start(&accounts, &recorder, "int-1", "rec-1").unwrap();
        codetrial::recording::record_egress_id(
            &accounts,
            recorder.clock.as_ref(),
            &recording.id,
            "EG_live",
        )
        .unwrap();

        scratch
            .open()
            .execute(
                "UPDATE interviews SET consent_withdrawn_at = 50 WHERE id = 'int-1'",
                [],
            )
            .unwrap();

        // A withdrawn interview cannot start another recording, or the
        // candidate who just said no could reload into a second one.
        assert_eq!(
            start(&accounts, &recorder, "int-1", "rec-other"),
            Ok(recording_by_id(&accounts, &recording.id).unwrap().unwrap()),
            "the existing recording is returned rather than a second one created"
        );
        scratch
            .open()
            .execute("DELETE FROM recordings WHERE id = 'rec-1'", [])
            .unwrap();
        assert_eq!(
            start(&accounts, &recorder, "int-1", "rec-again"),
            Err(StartRefusal::NoConsent),
            "and with no recording in the way, consent is what refuses"
        );

        // The stop itself: `failed`, not `finalizing`. The recording is not
        // finished, it is abandoned, and the file it produced is scheduled for
        // deletion rather than delivered. Driven through `stop_recording` here
        // because this file has no router; `consent_withdrawal_stops_egress` in
        // `tests/web.rs` drives the same thing through the route that a
        // candidate actually reaches.
        let connection = scratch.open();
        connection
            .execute(
                "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, egress_id, created_at, updated_at
        ) VALUES ('rec-live', 1, 'int-1', 'interview-abc12345', 'int-1',
            'one@example.test', 'recording', 'EG_live', 20, 20)
        ",
                [],
            )
            .unwrap();
        let live = recording_by_id(&accounts, "rec-live").unwrap().unwrap();
        let stopped = codetrial::recording::stop_recording(
            &accounts,
            &recorder,
            &live,
            RecordingState::Failed,
            Some("consent_withdrawn"),
        )
        .await
        .unwrap();
        assert_eq!(stopped, (RecordingState::Failed, true));
        assert_eq!(
            provider.stops(),
            vec!["EG_live@interview-abc12345".to_string()]
        );
        assert_eq!(
            recording_by_id(&accounts, "rec-live")
                .unwrap()
                .unwrap()
                .error
                .as_deref(),
            Some("consent_withdrawn"),
            "the reason is what a later deletion is justified by"
        );
    }

    #[tokio::test]
    async fn lifecycle_normal_end_stops() {
        let (_scratch, accounts, provider, _clock, recorder) = harness("normal-end");
        let recording = start(&accounts, &recorder, "int-1", "rec-1").unwrap();
        codetrial::recording::record_egress_id(
            &accounts,
            recorder.clock.as_ref(),
            &recording.id,
            "EG_live",
        )
        .unwrap();
        let recording = recording_by_id(&accounts, "rec-1").unwrap().unwrap();

        let stopped = codetrial::recording::stop_recording(
            &accounts,
            &recorder,
            &recording,
            RecordingState::Finalizing,
            None,
        )
        .await
        .unwrap();
        assert_eq!(stopped, (RecordingState::Finalizing, true));
        assert_eq!(
            provider.stops(),
            vec!["EG_live@interview-abc12345".to_string()]
        );

        // The other normal-end path arriving second changes nothing, which is
        // what lets a browser close and a webhook be lost independently. The
        // provider is not asked again either: only the caller that moved the
        // row talks to it, so `/end` racing a `room_finished` webhook sends one
        // stop rather than two.
        let recording = recording_by_id(&accounts, "rec-1").unwrap().unwrap();
        let again = codetrial::recording::stop_recording(
            &accounts,
            &recorder,
            &recording,
            RecordingState::Finalizing,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            again,
            (RecordingState::Finalizing, false),
            "the second caller did not move it, and has to be told so"
        );
        assert_eq!(
            provider.stops(),
            vec!["EG_live@interview-abc12345".to_string()],
            "one stop, not two"
        );
    }

    /// A stop that lands while the provider is still answering the start.
    ///
    /// The id used to be written only from `starting`, so the row kept the
    /// stop and threw away the handle: the Egress job it named could then not
    /// be stopped by anything, and it ran until LiveKit's own limit.
    #[tokio::test]
    async fn lifecycle_a_stop_before_the_start_response_keeps_the_handle() {
        let (_scratch, accounts, _provider, _clock, recorder) = harness("stop-before-start");
        start(&accounts, &recorder, "int-1", "rec-1").unwrap();

        // The stop arrives first, with no egress id to stop yet.
        transition(
            &accounts,
            recorder.clock.as_ref(),
            "rec-1",
            RecordingState::Finalizing,
            None,
        )
        .unwrap();

        // Then the provider answers.
        codetrial::recording::record_egress_id(
            &accounts,
            recorder.clock.as_ref(),
            "rec-1",
            "EG_late",
        )
        .unwrap();

        let recording = recording_by_id(&accounts, "rec-1").unwrap().unwrap();
        assert_eq!(
            recording.egress_id.as_deref(),
            Some("EG_late"),
            "the handle is kept whatever the state, or nothing can stop the job"
        );
        assert_eq!(
            recording.state,
            RecordingState::Finalizing,
            "and the late answer does not undo the stop"
        );
    }

    /// A recording that ends while the provider is still answering its start.
    ///
    /// Both the route and the sweeper's retry have this window: consent can be
    /// withdrawn, a room can finish, a tab can close, all while
    /// `StartRoomCompositeEgress` is in flight. The row that comes back is
    /// terminal with a live job attached, and something has to stop it.
    #[tokio::test]
    async fn lifecycle_a_job_that_outlived_its_recording_is_stopped() {
        let (_scratch, accounts, provider, clock, recorder) = harness("outlived");
        start(&accounts, &recorder, "int-1", "rec-1").unwrap();

        // A normal end, not an abandonment. `finalizing` is the case that hid:
        // the pipeline still owes this recording work, and the provider has
        // already been asked to stop, so a job attached afterwards is one whose
        // stop request arrived before it had an id to name.
        transition(
            &accounts,
            recorder.clock.as_ref(),
            "rec-1",
            RecordingState::Finalizing,
            None,
        )
        .unwrap();

        // Then the answer lands. `settle_new_egress` is what both the route and
        // the sweeper's retry call at exactly this point.
        codetrial::recording::record_egress_id(
            &accounts,
            recorder.clock.as_ref(),
            "rec-1",
            "EG_late",
        )
        .unwrap();
        codetrial::recording::settle_new_egress(&accounts, &recorder, "rec-1", "EG_late").await;

        assert_eq!(
            provider.stops(),
            vec!["EG_late@interview-abc12345".to_string()],
            "a job handed to a recording that already ended has to be stopped"
        );

        // And written down, or every sweep from here on asks the provider to
        // stop a job it already stopped.
        clock.advance(60);
        sweep_recordings(&accounts, &recorder).await;
        assert_eq!(provider.stops().len(), 1, "asked once, not once a minute");
    }

    /// A stop that failed after the row moved leaves a job nobody is watching.
    ///
    /// `failed` is terminal, so no state will ever say the provider is still
    /// running. The missing `stopped_at` is what says it, and the sweep is what
    /// asks again.
    #[tokio::test]
    async fn lifecycle_a_failed_stop_is_asked_again() {
        let (_scratch, accounts, provider, _clock, recorder) = harness("failed-stop");
        let recording = start(&accounts, &recorder, "int-1", "rec-1").unwrap();
        codetrial::recording::record_egress_id(
            &accounts,
            recorder.clock.as_ref(),
            &recording.id,
            "EG_live",
        )
        .unwrap();
        let recording = recording_by_id(&accounts, "rec-1").unwrap().unwrap();

        *provider.refuse_stop.lock().unwrap() = true;
        assert!(
            codetrial::recording::stop_recording(
                &accounts,
                &recorder,
                &recording,
                RecordingState::Failed,
                Some("consent_withdrawn"),
            )
            .await
            .is_err(),
            "the provider refused"
        );
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Failed);
        assert_eq!(provider.stops().len(), 1);

        // The row is terminal and the job is not. The sweep asks again, and
        // keeps asking until the provider agrees.
        sweep_recordings(&accounts, &recorder).await;
        assert_eq!(provider.stops().len(), 2, "a failed stop is asked again");

        *provider.refuse_stop.lock().unwrap() = false;
        sweep_recordings(&accounts, &recorder).await;
        assert_eq!(provider.stops().len(), 3);

        sweep_recordings(&accounts, &recorder).await;
        assert_eq!(
            provider.stops().len(),
            3,
            "and stops asking once it has agreed"
        );
    }

    /// A start whose id was never written down must not become two Egress jobs.
    ///
    /// The provider already has a job for the room, and starting again would
    /// make a second: two bills, two files, and neither stoppable by a
    /// candidate withdrawing consent.
    #[tokio::test]
    async fn lifecycle_a_lost_egress_id_is_adopted_rather_than_restarted() {
        let (_scratch, accounts, provider, clock, recorder) = harness("adopt");
        start(&accounts, &recorder, "int-1", "rec-1").unwrap();
        *provider.adopt.lock().unwrap() = Some("EG_orphan".to_string());

        clock.advance(60);
        assert_eq!(sweep_recordings(&accounts, &recorder).await.retried, 1);
        assert_eq!(provider.starts(), 0, "no second job");
        assert_eq!(
            recording_by_id(&accounts, "rec-1")
                .unwrap()
                .unwrap()
                .egress_id
                .as_deref(),
            Some("EG_orphan"),
            "the job that was already running is the one this recording owns"
        );
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Recording);
    }

    /// A provider that refuses the first attempt.
    ///
    /// The contract promises one minute, then five, then fifteen. A row moved
    /// straight to `failed` is a row that schedule never sees, so a refused
    /// start stays `starting` with a retry spent.
    #[tokio::test]
    async fn lifecycle_a_refused_start_keeps_its_retries() {
        let (_scratch, accounts, provider, clock, recorder) = harness("refused-start");
        start(&accounts, &recorder, "int-1", "rec-1").unwrap();
        *provider.refuse_start.lock().unwrap() = true;

        for expected in 1..=3 {
            clock.advance(codetrial::recording::RETRY_BACKOFF_SECONDS[expected - 1]);
            assert_eq!(sweep_recordings(&accounts, &recorder).await.retried, 0);
            assert_eq!(provider.starts(), expected);
            let recording = recording_by_id(&accounts, "rec-1").unwrap().unwrap();
            assert_eq!(recording.state, RecordingState::Starting);
            assert_eq!(recording.retries, expected as i64);
        }

        // Out of attempts, and now only the stale rule is left. A provider that
        // has refused three times over sixteen minutes is not going to answer
        // for an interview that will be over by then.
        clock.advance(codetrial::recording::STALE_SECONDS);
        assert_eq!(sweep_recordings(&accounts, &recorder).await.failed, 1);
        assert_eq!(provider.starts(), 3, "no fourth attempt");
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Failed);
    }

    #[tokio::test]
    async fn lifecycle_restart_resumes() {
        let (_scratch, accounts, provider, clock, recorder) = harness("restart");
        start(&accounts, &recorder, "int-1", "rec-1").unwrap();
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Starting);

        // Nothing to do yet: the row was touched a moment ago and the first
        // retry is a minute out.
        assert_eq!(sweep_recordings(&accounts, &recorder).await.retried, 0);
        assert_eq!(provider.starts(), 0);

        // A minute on, the start is attempted. This is the process that died
        // between writing `starting` and calling the provider.
        clock.advance(60);
        assert_eq!(sweep_recordings(&accounts, &recorder).await.retried, 1);
        assert_eq!(provider.starts(), 1);
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Recording);
        assert_eq!(
            recording_by_id(&accounts, "rec-1")
                .unwrap()
                .unwrap()
                .egress_id
                .as_deref(),
            Some("EG_fake1")
        );

        // Fifteen minutes is not enough for a `recording` row. LiveKit sends
        // `egress_updated` on a status change and not as a heartbeat, so a
        // healthy forty-minute interview touches its row once and not again;
        // the fifteen-minute rule applied here failed every recording that
        // outlived its own first quarter of an hour.
        clock.advance(codetrial::recording::STALE_SECONDS);
        assert_eq!(sweep_recordings(&accounts, &recorder).await.failed, 0);
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Recording);

        // What a recording cannot legitimately do is outlive the maximum this
        // server configured, plus enough for the provider to finish writing.
        clock.advance(
            codetrial::recording::stale_after(
                RecordingState::Recording,
                recorder.config.max_minutes,
            ) - codetrial::recording::STALE_SECONDS,
        );
        let outcome = sweep_recordings(&accounts, &recorder).await;
        assert_eq!(outcome.failed, 1);
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Failed);
        assert_eq!(
            recording_by_id(&accounts, "rec-1")
                .unwrap()
                .unwrap()
                .error
                .as_deref(),
            Some("abandoned")
        );
        assert_eq!(
            provider.stops(),
            vec!["EG_fake1@interview-abc12345".to_string()]
        );
    }

    #[tokio::test]
    async fn lifecycle_kill_switch_stops_what_is_already_running() {
        let (_scratch, accounts, provider, _clock, mut recorder) = harness("kill-switch");
        let recording = start(&accounts, &recorder, "int-1", "rec-1").unwrap();
        codetrial::recording::record_egress_id(
            &accounts,
            recorder.clock.as_ref(),
            &recording.id,
            "EG_live",
        )
        .unwrap();

        recorder.config.kill_switch = true;
        assert_eq!(
            start(&accounts, &recorder, "int-2", "rec-2"),
            Err(StartRefusal::KillSwitch),
            "the switch outranks the per-account limit: recording is off, not busy"
        );

        // A switch that only refuses new starts is not a kill switch: an
        // operator who throws it while interviews are running means them too,
        // and the sweep does not wait for a row to go stale first.
        let outcome = sweep_recordings(&accounts, &recorder).await;
        assert_eq!(outcome.failed, 1);
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Failed);
        assert_eq!(
            recording_by_id(&accounts, "rec-1")
                .unwrap()
                .unwrap()
                .error
                .as_deref(),
            Some("kill_switch")
        );
        assert_eq!(
            provider.stops(),
            vec!["EG_live@interview-abc12345".to_string()]
        );

        assert_eq!(
            start(&accounts, &recorder, "int-2", "rec-2"),
            Err(StartRefusal::KillSwitch),
            "and it keeps refusing once nothing is running"
        );
    }

    #[tokio::test]
    async fn duplicate_start() {
        let (scratch, accounts, _provider, _clock, recorder) = harness("duplicate-start");
        let (first, mine) = started(&accounts, &recorder, "int-1", "rec-1").unwrap();
        assert!(
            mine,
            "the caller that inserted the row is the one that starts"
        );

        // A second start finds the first caller's row rather than creating a
        // second Egress job. `interview_id` is UNIQUE, so this is the schema
        // answering rather than a check somebody remembered to write.
        let (second, mine) = started(&accounts, &recorder, "int-1", "rec-2").unwrap();
        assert_eq!(second, first);
        assert!(
            !mine,
            "and the second caller must not also call the provider, or one \
             interview gets two Egress jobs"
        );
        assert_eq!(
            scratch
                .open()
                .query_row("SELECT COUNT(*) FROM recordings", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );

        // And somebody else's interview is refused rather than adopted.
        assert_eq!(
            begin_recording(
                &accounts,
                recorder.clock.as_ref(),
                "int-1",
                2,
                "rec-3",
                "two@example.test",
                false,
            )
            .map(|(recording, _)| recording),
            Err(StartRefusal::NoConsent)
        );
    }

    #[tokio::test]
    async fn duplicate_webhook() {
        let (_scratch, accounts, _provider, clock, _recorder) = harness("duplicate-webhook");

        assert!(claim_webhook_event(&accounts, "EV_one", clock.now()).unwrap());

        // Claimed and not yet applied. A retry gets another go, because the
        // first attempt did not finish and deleting the claim on failure was a
        // write that could itself fail.
        assert!(
            claim_webhook_event(&accounts, "EV_one", clock.now() + 65).unwrap(),
            "an event nobody finished applying is not a duplicate"
        );

        codetrial::recording::mark_webhook_applied(&accounts, "EV_one", clock.now()).unwrap();
        assert!(
            !claim_webhook_event(&accounts, "EV_one", clock.now() + 130).unwrap(),
            "a retry carries a new createdAt and the same id, so the id is the key"
        );
        assert!(claim_webhook_event(&accounts, "EV_two", clock.now()).unwrap());

        // The ledger is bounded. LiveKit gives up retrying long before this.
        codetrial::recording::prune_webhook_events(
            &accounts,
            clock.now() + codetrial::recording::EVENT_RETENTION_SECONDS,
        )
        .unwrap();
        assert!(
            claim_webhook_event(&accounts, "EV_one", clock.now()).unwrap(),
            "a pruned event is forgotten, which is what bounded means"
        );
    }
}

/// Every way a recording can fail, driven through an injected provider.
///
/// None of these can be produced against LiveKit or Google Drive on demand, so
/// the failure is the thing being injected and the terminal state is the thing
/// being asserted. What each test pins is that the recording ends up somewhere
/// a person can act on, with a reason that says which action.
mod failure {
    use std::sync::{Arc, Mutex};

    use codetrial::accounts::Accounts;
    use codetrial::recording::{
        Clock, DeliveryProvider, Failure, Recorder, Recording, RecordingState, STALE_SECONDS,
        completed_with_output, delete_recording, deliver_recording, recording_by_id, stale_after,
        sweep_recordings, transition,
    };
    use serde_json::json;

    use super::Scratch;
    use super::lifecycle::{TestClock, harness, start, state_of};

    /// Drive, as far as this pipeline is concerned: three calls, each of which
    /// can be told to refuse.
    #[derive(Default)]
    struct FakeDelivery {
        /// A database to move the row in while `transfer` is running, standing
        /// in for a withdrawal that lands mid-delivery. It is the only way to
        /// reach that interleaving from a test.
        preempt: Mutex<Option<std::path::PathBuf>>,
        /// What to run there. The default moves the row out of `transferring`,
        /// which is a withdrawal; a test that wants a competing delivery says
        /// so here.
        preempt_sql: Mutex<Option<String>>,
        /// The same, applied during `share` rather than `transfer`, for the
        /// race at the permission boundary.
        preempt_share: Mutex<Option<std::path::PathBuf>>,
        refuse_transfer: Mutex<bool>,
        refuse_share: Mutex<bool>,
        refuse_delete: Mutex<bool>,
        refuse_revoke: Mutex<bool>,
        refuse_object_delete: Mutex<bool>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeDelivery {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl DeliveryProvider for FakeDelivery {
        fn transfer<'a>(
            &'a self,
            _gcs_object: &'a str,
            _filename: &'a str,
            _recording_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
        {
            Box::pin(async move {
                self.calls.lock().unwrap().push("transfer".to_string());
                if let Some(path) = self.preempt.lock().unwrap().as_ref() {
                    let statement = self.preempt_sql.lock().unwrap().clone().unwrap_or_else(|| {
                        "UPDATE recordings SET state = 'failed', error = 'consent_withdrawn' WHERE state = 'transferring'"
                            .to_string()
                    });
                    rusqlite::Connection::open(path)
                        .unwrap()
                        .execute(&statement, [])
                        .unwrap();
                }
                if *self.refuse_transfer.lock().unwrap() {
                    return Err("the upload was refused".to_string());
                }
                Ok("drive-file-1".to_string())
            })
        }

        fn share<'a>(
            &'a self,
            _drive_file_id: &'a str,
            _recipient_email: &'a str,
            _expires_at: i64,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>
        {
            Box::pin(async move {
                self.calls.lock().unwrap().push("share".to_string());
                if let Some(path) = self.preempt_share.lock().unwrap().as_ref() {
                    rusqlite::Connection::open(path)
                        .unwrap()
                        .execute(
                            "UPDATE recordings SET state = 'failed', error = 'consent_withdrawn' WHERE state = 'transferring'",
                            [],
                        )
                        .unwrap();
                }
                if *self.refuse_share.lock().unwrap() {
                    return Err("the permission was refused".to_string());
                }
                Ok("permission-1".to_string())
            })
        }

        fn revoke_and_delete<'a>(
            &'a self,
            drive_file_id: Option<&'a str>,
            permission: Option<(&'a str, &'a str)>,
            gcs_object: Option<&'a str>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>
        {
            Box::pin(async move {
                // The arguments, not just the call. A cleanup that removed the
                // Drive file and left the staged bytes would pass an assertion
                // that only counted calls. The permission carries the file it
                // is on, which is not always the file being deleted.
                self.calls.lock().unwrap().push(format!(
                    "revoke_and_delete file={} permission={} on={} object={}",
                    drive_file_id.unwrap_or("-"),
                    permission.map_or("-", |(_, id)| id),
                    permission.map_or("-", |(file, _)| file),
                    gcs_object.unwrap_or("-")
                ));
                if *self.refuse_delete.lock().unwrap() {
                    return Err("the file would not delete".to_string());
                }
                Ok(())
            })
        }

        fn revoke<'a>(
            &'a self,
            drive_file_id: &'a str,
            permission_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>
        {
            Box::pin(async move {
                self.calls.lock().unwrap().push(format!(
                    "revoke on={drive_file_id} permission={permission_id}"
                ));
                if *self.refuse_revoke.lock().unwrap() {
                    return Err("the permission would not revoke".to_string());
                }
                Ok(())
            })
        }

        fn delete_file<'a>(
            &'a self,
            drive_file_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>
        {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("delete_file file={drive_file_id}"));
                if *self.refuse_delete.lock().unwrap() {
                    return Err("the file would not delete".to_string());
                }
                Ok(())
            })
        }

        fn delete_object<'a>(
            &'a self,
            gcs_object: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>
        {
            Box::pin(async move {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("delete_object object={gcs_object}"));
                if *self.refuse_object_delete.lock().unwrap() {
                    return Err("the staged object would not delete".to_string());
                }
                Ok(())
            })
        }
    }

    /// A recording that reached `transferring`, which is where delivery starts.
    async fn transferring(
        accounts: &Arc<Accounts>,
        recorder: &Recorder,
        clock: &TestClock,
    ) -> Recording {
        let recording = start(accounts, recorder, "int-1", "rec-1").unwrap();
        codetrial::recording::record_egress_id(
            accounts,
            recorder.clock.as_ref(),
            &recording.id,
            "EG_done",
        )
        .unwrap();

        // The webhook marks the job stopped before it moves the row, and
        // retention will not touch a recording whose Egress job was never
        // confirmed stopped, so a helper that skipped this would be building a
        // state the pipeline does not produce.
        codetrial::recording::mark_stopped(accounts, recorder.clock.as_ref(), &recording.id)
            .unwrap();
        transition(
            accounts,
            recorder.clock.as_ref(),
            &recording.id,
            RecordingState::Transferring,
            None,
        )
        .unwrap();
        let _ = clock;
        recording_by_id(accounts, &recording.id).unwrap().unwrap()
    }

    #[tokio::test]
    async fn failure_start_gives_up_after_its_retries() {
        let (_scratch, accounts, provider, clock, recorder) = harness("failure-start");
        start(&accounts, &recorder, "int-1", "rec-1").unwrap();
        *provider.refuse_start.lock().unwrap() = true;

        for delay in codetrial::recording::RETRY_BACKOFF_SECONDS {
            clock.advance(delay);
            sweep_recordings(&accounts, &recorder).await;
        }
        assert_eq!(provider.starts(), 3, "one minute, then five, then fifteen");

        // Out of retries ends it, without waiting for the stale rule. A
        // provider that refused every time is a different ending from nobody
        // having said anything, and the reason has to say which. The minute is
        // the sweeper's own floor: it does not look at a row somebody touched
        // moments ago, and the third attempt touched this one.
        clock.advance(60);
        assert_eq!(sweep_recordings(&accounts, &recorder).await.failed, 1);
        assert_eq!(provider.starts(), 3, "and no fourth attempt");
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Failed);

        let recording = recording_by_id(&accounts, "rec-1").unwrap().unwrap();
        let failure = Failure::parse(recording.error.as_deref().unwrap()).unwrap();
        assert_eq!(failure, Failure::Start);

        // The reason names the recovery. A recording that never produced
        // anything can be started again, and saying so is the difference
        // between a status and an instruction.
        assert_eq!(failure.recovery(), "start_again");
        let _ = STALE_SECONDS;
    }

    #[tokio::test]
    async fn failure_partial_output_is_not_a_completion() {
        // The provider says it finished and hands back nothing worth moving.
        // Sending that through the transfer shared a zero-byte video.
        let expected = "codetrial/rec.mp4";
        let ended = |files: serde_json::Value| {
            json!({
                "event": "egress_ended",
                "egressInfo": { "status": "EGRESS_COMPLETE", "fileResults": files }
            })
        };

        assert!(!completed_with_output(&ended(json!([])), expected));
        assert!(
            !completed_with_output(
                &ended(json!([{ "filename": expected, "size": "0" }])),
                expected
            ),
            "no bytes is not a recording"
        );
        assert!(
            !completed_with_output(
                &ended(json!([{ "filename": "codetrial/rec.json", "size": "412" }])),
                expected
            ),
            "a manifest is a file with a name and a size, and delivering it is delivering the wrong thing"
        );
        assert!(completed_with_output(
            &ended(json!([{ "filename": expected, "size": "30750000" }])),
            expected
        ));

        // `int64` crosses protojson as a string, so a quoted count is the shape
        // this pipeline receives. A number is accepted too: it is a serializer
        // setting on somebody else's service, and throwing a recording away
        // over a configuration nobody here controls is the worse mistake.
        assert!(completed_with_output(
            &ended(json!([{ "filename": expected, "size": 30_750_000 }])),
            expected
        ));

        assert_eq!(Failure::PartialOutput.recovery(), "start_again");
        assert_eq!(
            Failure::LimitReached.recovery(),
            "wait_for_operator",
            "an allowance the candidate cannot raise is not one they can retry past"
        );
    }

    #[tokio::test]
    async fn failure_webhook_that_never_arrives_still_ends() {
        let (_scratch, accounts, provider, clock, recorder) = harness("failure-webhook");
        let recording = start(&accounts, &recorder, "int-1", "rec-1").unwrap();
        codetrial::recording::record_egress_id(
            &accounts,
            recorder.clock.as_ref(),
            &recording.id,
            "EG_silent",
        )
        .unwrap();

        // Nothing ever says what happened to it. Without the sweeper this row
        // holds the account's one active slot forever.
        clock.advance(stale_after(
            RecordingState::Recording,
            recorder.config.max_minutes,
        ));
        assert_eq!(sweep_recordings(&accounts, &recorder).await.failed, 1);
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Failed);
        assert_eq!(
            recording_by_id(&accounts, "rec-1")
                .unwrap()
                .unwrap()
                .error
                .as_deref(),
            Some(Failure::LostWebhook.as_str())
        );
        assert_eq!(
            provider.stops(),
            vec!["EG_silent@interview-abc12345".to_string()],
            "and the job it was waiting on is stopped rather than left running"
        );
    }

    /// The delivery queue: the work between a recording with a file and a
    /// candidate who can read it.
    ///
    /// What these are about is the claim. Without one, two workers both see no
    /// `drive_file_id`, both upload, and the loser's file sits in the Shared
    /// Drive with nothing naming it and nothing able to delete it.
    mod transfer {
        use codetrial::recording::{
            Clock, DELIVERY_BACKOFF_SECONDS, DeliveryOutcome, Failure, RecordingState,
            claim_delivery, delivery_attempts, enqueue_delivery, recording_by_id,
            run_delivery_queue,
        };

        use super::super::lifecycle::{harness, start, state_of};
        use super::{FakeDelivery, transferring};

        #[tokio::test]
        async fn transfer_no_duplicate_drive_file() {
            let (_scratch, accounts, _provider, clock, recorder) = harness("transfer-duplicate");
            let recording = transferring(&accounts, &recorder, &clock).await;
            enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();

            // The second start is what a webhook LiveKit sent twice looks like.
            enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();
            let queued: i64 = _scratch
                .open()
                .query_row("SELECT COUNT(*) FROM delivery_queue", [], |row| row.get(0))
                .unwrap();
            assert_eq!(queued, 1, "one delivery, however many times it was queued");

            // One claim, then nothing: the second worker finds the row taken
            // and goes away rather than uploading beside the first.
            let first = claim_delivery(&accounts, clock.now()).unwrap();
            assert_eq!(first.map(|(row, _)| row.id), Some(recording.id.clone()));
            assert_eq!(
                claim_delivery(&accounts, clock.now())
                    .unwrap()
                    .map(|(row, _)| row.id),
                None,
                "a claimed row is not claimable again"
            );

            // The claim a worker was replaced under is not one it may write
            // with. Without this, a worker whose claim expired mid-upload could
            // delete the row its replacement is holding, and the replacement's
            // delivery would finish into a queue that had forgotten it.
            codetrial::recording::finish_delivery_work(&accounts, &recording.id, "not-my-claim")
                .unwrap();

            // Nor may it reschedule or fail it. "Not ours" is a third answer,
            // because failing the recording here would stop a delivery another
            // worker is in the middle of and delete the file it uploaded.
            assert_eq!(
                codetrial::recording::reschedule_delivery(
                    &accounts,
                    &recording.id,
                    "not-my-claim",
                    "drive_failed",
                    clock.now(),
                )
                .unwrap(),
                codetrial::recording::Reschedule::NotOurs
            );
            assert_eq!(
                delivery_attempts(&accounts, &recording.id).unwrap(),
                Some(1)
            );

            // And the delivery itself, through the worker, leaves one file.
            let delivery = FakeDelivery::default();
            assert_eq!(
                run_delivery_queue(&accounts, &recorder, &delivery).await,
                DeliveryOutcome::default(),
                "nothing is due while the claim is fresh"
            );
            clock.advance(codetrial::recording::DELIVERY_CLAIM_SECONDS + 1);
            assert_eq!(
                run_delivery_queue(&accounts, &recorder, &delivery).await,
                DeliveryOutcome {
                    delivered: 1,
                    ..DeliveryOutcome::default()
                }
            );
            assert_eq!(state_of(&accounts, &recording.id), RecordingState::Ready);
            assert_eq!(
                delivery.calls(),
                vec!["transfer".to_string(), "share".to_string()],
                "one upload and one share, not two of either"
            );
            assert_eq!(
                delivery_attempts(&accounts, &recording.id).unwrap(),
                None,
                "a delivered recording owes no more work"
            );
        }

        #[tokio::test]
        async fn transfer_resumes_after_restart() {
            let (scratch, accounts, _provider, clock, recorder) = harness("transfer-restart");
            let recording = transferring(&accounts, &recorder, &clock).await;
            enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();

            // A worker that uploaded and then died with the process: the file
            // id is written down, the claim is still held, and nothing has
            // shared it.
            let delivery = FakeDelivery::default();
            *delivery.refuse_share.lock().unwrap() = true;
            assert_eq!(
                run_delivery_queue(&accounts, &recorder, &delivery).await,
                DeliveryOutcome {
                    retrying: 1,
                    ..DeliveryOutcome::default()
                }
            );
            let row = recording_by_id(&accounts, &recording.id).unwrap().unwrap();
            assert_eq!(
                state_of(&accounts, &recording.id),
                RecordingState::Transferring
            );
            let file: Option<String> = scratch
                .open()
                .query_row(
                    "SELECT drive_file_id FROM recordings WHERE id = ?1",
                    [&recording.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(file.as_deref(), Some("drive-file-1"));
            assert_eq!(row.error.as_deref(), Some(Failure::Drive.as_str()));

            // The retry is not due yet, and then it is.
            assert_eq!(
                run_delivery_queue(&accounts, &recorder, &delivery).await,
                DeliveryOutcome::default(),
                "a backoff nothing waits for is not a backoff"
            );
            clock.advance(DELIVERY_BACKOFF_SECONDS[0]);
            *delivery.refuse_share.lock().unwrap() = false;
            assert_eq!(
                run_delivery_queue(&accounts, &recorder, &delivery).await,
                DeliveryOutcome {
                    delivered: 1,
                    ..DeliveryOutcome::default()
                }
            );

            assert_eq!(state_of(&accounts, &recording.id), RecordingState::Ready);
            assert_eq!(
                delivery
                    .calls()
                    .iter()
                    .filter(|call| *call == "transfer")
                    .count(),
                1,
                "the second attempt reused the file the first one uploaded"
            );
        }

        #[tokio::test]
        async fn transfer_gives_up_on_a_schedule() {
            let (_scratch, accounts, _provider, clock, recorder) = harness("transfer-schedule");
            let recording = transferring(&accounts, &recorder, &clock).await;
            enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();
            let delivery = FakeDelivery::default();
            *delivery.refuse_transfer.lock().unwrap() = true;

            // Three attempts, at zero, one minute and five.
            for backoff in DELIVERY_BACKOFF_SECONDS {
                assert_eq!(
                    run_delivery_queue(&accounts, &recorder, &delivery).await,
                    DeliveryOutcome {
                        retrying: 1,
                        ..DeliveryOutcome::default()
                    }
                );
                assert_eq!(
                    run_delivery_queue(&accounts, &recorder, &delivery).await,
                    DeliveryOutcome::default(),
                    "not due yet"
                );
                clock.advance(backoff);
            }
            assert_eq!(
                run_delivery_queue(&accounts, &recorder, &delivery).await,
                DeliveryOutcome {
                    failed: 1,
                    ..DeliveryOutcome::default()
                }
            );

            // Failed with the reason an operator can act on, rather than left
            // for the sweeper to abandon.
            assert_eq!(state_of(&accounts, &recording.id), RecordingState::Failed);
            let row = recording_by_id(&accounts, &recording.id).unwrap().unwrap();
            assert_eq!(row.error.as_deref(), Some(Failure::Drive.as_str()));
            assert_eq!(delivery_attempts(&accounts, &recording.id).unwrap(), None);
            assert_eq!(
                delivery.calls().len(),
                3,
                "three attempts, not one and not forever"
            );
        }

        #[tokio::test]
        async fn transfer_retries_a_delivery_an_operator_asked_for() {
            let (_scratch, accounts, _provider, clock, recorder) = harness("transfer-reopen");
            let recording = transferring(&accounts, &recorder, &clock).await;
            enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();
            let delivery = FakeDelivery::default();
            *delivery.refuse_transfer.lock().unwrap() = true;

            // Run it out of attempts, which is the state `drive_failed` leaves.
            for backoff in DELIVERY_BACKOFF_SECONDS {
                run_delivery_queue(&accounts, &recorder, &delivery).await;
                clock.advance(backoff);
            }
            run_delivery_queue(&accounts, &recorder, &delivery).await;
            assert_eq!(state_of(&accounts, &recording.id), RecordingState::Failed);
            assert_eq!(Failure::Drive.recovery(), "retry_delivery");

            // The recovery, performed: the row is queued again and the worker
            // reopens it. The bytes never left the staging bucket, which is why
            // this is a delivery to retry rather than an interview to record
            // again.
            *delivery.refuse_transfer.lock().unwrap() = false;
            enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();
            assert_eq!(
                run_delivery_queue(&accounts, &recorder, &delivery).await,
                DeliveryOutcome {
                    delivered: 1,
                    ..DeliveryOutcome::default()
                }
            );
            assert_eq!(state_of(&accounts, &recording.id), RecordingState::Ready);

            // And a recording that failed for any other reason stays failed: a
            // reopen for one code must not become a reopen for all of them.
            let other = start(&accounts, &recorder, "int-2", "rec-2").unwrap();
            codetrial::recording::transition(
                &accounts,
                recorder.clock.as_ref(),
                &other.id,
                RecordingState::Failed,
                Some(Failure::Egress.as_str()),
            )
            .unwrap();
            let before = delivery.calls().len();
            enqueue_delivery(&accounts, &other.id, clock.now()).unwrap();
            assert_eq!(
                run_delivery_queue(&accounts, &recorder, &delivery).await,
                DeliveryOutcome::default()
            );
            assert_eq!(state_of(&accounts, &other.id), RecordingState::Failed);
            assert_eq!(
                delivery.calls().len(),
                before,
                "an egress failure has no file in the bucket to deliver"
            );
        }

        #[tokio::test]
        async fn transfer_is_not_swept_while_it_is_queued() {
            // The sweeper's job is rows nothing is moving. A delivery takes up
            // to half an hour and touches nothing while it runs, and a second
            // one waits behind it, so a recording the queue still owes work on
            // is not stale however old its row looks.
            let (_scratch, accounts, _provider, clock, recorder) = harness("transfer-not-stale");
            let recording = transferring(&accounts, &recorder, &clock).await;
            enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();
            clock.advance(codetrial::recording::STALE_SECONDS * 2);

            let stale =
                codetrial::recording::stale_active(&accounts, Some(clock.now() - 60)).unwrap();
            assert!(
                stale.iter().all(|row| row.id != recording.id),
                "a queued delivery is work in progress, not an abandoned row"
            );

            // The kill switch still takes it: a kill switch that waited for an
            // upload is not one.
            let killed = codetrial::recording::stale_active(&accounts, None).unwrap();
            assert!(killed.iter().any(|row| row.id == recording.id));
        }

        #[tokio::test]
        async fn transfer_requeues_a_delivery_nobody_is_doing() {
            // The transition to `transferring` and the queue insert are two
            // writes. A process that died between them leaves a recording that
            // owes a delivery and a queue that has never heard of it, and
            // without this the sweeper would abandon it fifteen minutes later.
            let (_scratch, accounts, _provider, clock, recorder) = harness("transfer-orphan");
            let recording = transferring(&accounts, &recorder, &clock).await;
            assert_eq!(delivery_attempts(&accounts, &recording.id).unwrap(), None);

            assert_eq!(
                codetrial::recording::requeue_orphaned_deliveries(&accounts, clock.now()).unwrap(),
                1
            );
            assert_eq!(
                codetrial::recording::requeue_orphaned_deliveries(&accounts, clock.now()).unwrap(),
                0,
                "a queue that already knows is not told twice"
            );

            // And the sweep repairs before it reads: a restart longer than the
            // stale window leaves exactly this row, and a sweep that read it as
            // stale first would fail the recording it had just queued.
            clock.advance(codetrial::recording::STALE_SECONDS * 2);
            codetrial::recording::sweep_recordings(&accounts, &recorder).await;
            assert_eq!(
                state_of(&accounts, &recording.id),
                RecordingState::Transferring
            );

            let delivery = FakeDelivery::default();
            assert_eq!(
                run_delivery_queue(&accounts, &recorder, &delivery).await,
                DeliveryOutcome {
                    delivered: 1,
                    ..DeliveryOutcome::default()
                }
            );
            assert_eq!(state_of(&accounts, &recording.id), RecordingState::Ready);
        }

        #[tokio::test]
        async fn transfer_skips_a_recording_that_moved_on() {
            let (scratch, accounts, _provider, clock, recorder) = harness("transfer-withdrawn");
            let recording = transferring(&accounts, &recorder, &clock).await;
            enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();

            // A withdrawal that landed while the delivery was queued. The media
            // is somebody else's problem now, and uploading it would hand out a
            // file that has been recalled.
            scratch
                .open()
                .execute(
                    "UPDATE recordings SET state = 'failed', error = 'consent_withdrawn' WHERE id = ?1",
                    [&recording.id],
                )
                .unwrap();

            let delivery = FakeDelivery::default();
            assert_eq!(
                run_delivery_queue(&accounts, &recorder, &delivery).await,
                DeliveryOutcome::default()
            );
            assert!(delivery.calls().is_empty(), "nothing was uploaded");
            assert_eq!(delivery_attempts(&accounts, &recording.id).unwrap(), None);
        }
    }

    #[tokio::test]
    async fn failure_drive_leaves_the_bytes_where_they_are() {
        let (_scratch, accounts, _provider, clock, recorder) = harness("failure-drive");
        let recording = transferring(&accounts, &recorder, &clock).await;
        let delivery = FakeDelivery::default();

        *delivery.refuse_transfer.lock().unwrap() = true;
        assert_eq!(
            deliver_recording(
                &accounts,
                &recorder,
                &delivery,
                &recording,
                "one@example.test",
                86_400,
            )
            .await,
            Err(Failure::Drive)
        );

        // Still `transferring`, which is what makes the recovery reachable: a
        // failure that moved the row to `failed` would advertise another
        // delivery attempt from a state no transition can leave.
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Transferring);
        let row = recording_by_id(&accounts, "rec-1").unwrap().unwrap();
        assert_eq!(row.error.as_deref(), Some(Failure::Drive.as_str()));
        assert_eq!(row.retries, 1, "the attempt is counted");

        // Another delivery, not another interview. The bytes are still in the
        // staging bucket, and telling a candidate to record again would be
        // telling them to be recorded twice.
        assert_eq!(Failure::Drive.recovery(), "retry_delivery");
        assert_eq!(delivery.calls(), vec!["transfer".to_string()]);

        // And the retry works, from the state the failure left behind.
        *delivery.refuse_transfer.lock().unwrap() = false;
        let row = recording_by_id(&accounts, "rec-1").unwrap().unwrap();
        assert_eq!(
            deliver_recording(
                &accounts,
                &recorder,
                &delivery,
                &row,
                "one@example.test",
                86_400,
            )
            .await,
            Ok(RecordingState::Ready),
            "retry_delivery has to be a recovery something can perform"
        );
    }

    #[tokio::test]
    async fn failure_drive_after_the_upload_keeps_the_file_id() {
        let (scratch, accounts, _provider, clock, recorder) = harness("failure-share");
        let recording = transferring(&accounts, &recorder, &clock).await;
        let delivery = FakeDelivery::default();
        *delivery.refuse_share.lock().unwrap() = true;

        assert_eq!(
            deliver_recording(
                &accounts,
                &recorder,
                &delivery,
                &recording,
                "one@example.test",
                86_400,
            )
            .await,
            Err(Failure::Drive)
        );

        // The file is in the Shared Drive. A row that forgot its id is a file
        // nothing can find, and nothing can delete. It stays `transferring`,
        // because the share is the step that failed and the upload is not worth
        // doing again.
        let drive_file: Option<String> = scratch
            .open()
            .query_row(
                "SELECT drive_file_id FROM recordings WHERE id = 'rec-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(drive_file.as_deref(), Some("drive-file-1"));
    }

    /// A retry after a share failure does not upload the file again.
    ///
    /// The first attempt already put it in the Shared Drive. Uploading again
    /// leaves that one there with nothing naming it, which is a file nobody can
    /// find and nobody can delete.
    #[tokio::test]
    async fn failure_drive_retry_keeps_the_file_it_already_uploaded() {
        let (_scratch, accounts, _provider, clock, recorder) = harness("failure-retry");
        let recording = transferring(&accounts, &recorder, &clock).await;
        let delivery = FakeDelivery::default();
        *delivery.refuse_share.lock().unwrap() = true;
        assert!(
            deliver_recording(
                &accounts,
                &recorder,
                &delivery,
                &recording,
                "one@example.test",
                86_400,
            )
            .await
            .is_err()
        );

        *delivery.refuse_share.lock().unwrap() = false;
        let row = recording_by_id(&accounts, "rec-1").unwrap().unwrap();
        assert_eq!(
            deliver_recording(
                &accounts,
                &recorder,
                &delivery,
                &row,
                "one@example.test",
                86_400,
            )
            .await,
            Ok(RecordingState::Ready)
        );
        assert_eq!(
            delivery
                .calls()
                .iter()
                .filter(|call| *call == "transfer")
                .count(),
            1,
            "one upload, however many times the share is attempted"
        );
    }

    /// A recording that already ended reaches no provider at all.
    ///
    /// The staged object is written down first and only for a `transferring`
    /// row, so a delivery called after a withdrawal stops before it can make a
    /// Drive copy of a recording nobody consented to keep.
    #[tokio::test]
    async fn failure_drive_refuses_a_recording_that_already_ended() {
        let (scratch, accounts, _provider, clock, recorder) = harness("failure-ended");
        let recording = transferring(&accounts, &recorder, &clock).await;
        scratch
            .open()
            .execute(
                "UPDATE recordings SET state = 'failed', error = 'consent_withdrawn' WHERE id = 'rec-1'",
                [],
            )
            .unwrap();

        let delivery = FakeDelivery::default();
        assert_eq!(
            deliver_recording(
                &accounts,
                &recorder,
                &delivery,
                &recording,
                "one@example.test",
                86_400,
            )
            .await,
            Err(Failure::Drive)
        );
        assert_eq!(
            delivery.calls(),
            Vec::<String>::new(),
            "nothing remote happens for a recording that has ended"
        );
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Failed);
    }

    /// A delivery that lost its row mid-flight takes what it made with it.
    ///
    /// The winner is a withdrawal or a deletion, so the file this attempt
    /// created belongs to a recording that has ended, and nothing is going to
    /// come back for it.
    #[tokio::test]
    async fn failure_drive_that_loses_its_row_cleans_up_after_itself() {
        let (scratch, accounts, _provider, clock, recorder) = harness("failure-lost");
        let recording = transferring(&accounts, &recorder, &clock).await;

        let delivery = FakeDelivery::default();
        *delivery.preempt.lock().unwrap() = Some(scratch.path().to_path_buf());

        assert_eq!(
            deliver_recording(
                &accounts,
                &recorder,
                &delivery,
                &recording,
                "one@example.test",
                86_400,
            )
            .await,
            Err(Failure::Drive)
        );
        let cleanup = delivery
            .calls()
            .into_iter()
            .find(|call| call.starts_with("revoke_and_delete"))
            .expect("what this attempt made goes with the recording that ended");
        assert!(cleanup.contains("file=drive-file-1"), "{cleanup}");
        assert!(
            cleanup.contains("object=codetrial/rec-1.mp4"),
            "the winner ended, so the staged bytes go with the file: {cleanup}"
        );
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Failed);
    }

    /// A delivery that lost to another delivery deletes only what it made.
    ///
    /// The staged object is what the winner is reading from, and a file the
    /// loser reused belongs to an earlier attempt. Deleting either because this
    /// call lost a race takes media from somebody who still wants it.
    #[tokio::test]
    async fn failure_drive_that_loses_to_another_delivery_keeps_the_shared_object() {
        let (scratch, accounts, _provider, clock, recorder) = harness("failure-shared");
        let recording = transferring(&accounts, &recorder, &clock).await;

        // The winner writes its own file id while this one is uploading, and
        // leaves the row transferring: another delivery is still working.
        let delivery = FakeDelivery::default();
        *delivery.preempt.lock().unwrap() = Some(scratch.path().to_path_buf());
        *delivery.preempt_sql.lock().unwrap() = Some(
            "UPDATE recordings SET drive_file_id = 'drive-winner' WHERE state = 'transferring'"
                .to_string(),
        );

        assert_eq!(
            deliver_recording(
                &accounts,
                &recorder,
                &delivery,
                &recording,
                "one@example.test",
                86_400,
            )
            .await,
            Err(Failure::Drive)
        );
        let cleanup = delivery
            .calls()
            .into_iter()
            .find(|call| call.starts_with("revoke_and_delete"))
            .expect("the file this attempt uploaded goes");
        assert!(cleanup.contains("file=drive-file-1"), "{cleanup}");
        assert!(
            cleanup.contains("object=-"),
            "the staged object is the winner's source, not this attempt's to delete: {cleanup}"
        );

        // And the winner's row is untouched. Recording this delivery's failure
        // would put its error and retry count on somebody else's delivery.
        let winner = recording_by_id(&accounts, "rec-1").unwrap().unwrap();
        assert_eq!(winner.error, None);
        assert_eq!(winner.retries, 0);
        let drive_file: Option<String> = scratch
            .open()
            .query_row(
                "SELECT drive_file_id FROM recordings WHERE id = 'rec-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(drive_file.as_deref(), Some("drive-winner"));
    }

    /// A delivery that reused a file and created its own permission revokes
    /// that permission without deleting the file it borrowed.
    ///
    /// Drive cannot revoke a permission without knowing which file it is on,
    /// and the file here belongs to an earlier attempt: revoking needs its id,
    /// deleting must not use it.
    #[tokio::test]
    async fn failure_drive_loser_revokes_its_permission_without_deleting_a_borrowed_file() {
        let (scratch, accounts, _provider, clock, recorder) = harness("failure-borrowed");
        let recording = transferring(&accounts, &recorder, &clock).await;

        // An earlier attempt already uploaded, so this one reuses that file.
        scratch
            .open()
            .execute(
                "UPDATE recordings SET drive_file_id = 'drive-earlier' WHERE id = 'rec-1'",
                [],
            )
            .unwrap();

        // The row moves while this attempt is sharing.
        let delivery = FakeDelivery::default();
        *delivery.preempt_share.lock().unwrap() = Some(scratch.path().to_path_buf());

        assert_eq!(
            deliver_recording(
                &accounts,
                &recorder,
                &delivery,
                &recording,
                "one@example.test",
                86_400,
            )
            .await,
            Err(Failure::Drive)
        );
        let cleanup = delivery
            .calls()
            .into_iter()
            .find(|call| call.starts_with("revoke_and_delete"))
            .expect("the permission this attempt granted has to be revoked");
        assert!(
            cleanup.contains("permission=permission-1") && cleanup.contains("on=drive-earlier"),
            "revoking needs the file the permission is on: {cleanup}"
        );
        assert!(
            cleanup.contains("file=-"),
            "and the borrowed file is not this attempt's to delete: {cleanup}"
        );
    }

    /// The same loss, against a winner that has already failed its own share.
    ///
    /// `drive_failed` means the queue will come back, so the staged object
    /// stays. The file this invocation uploaded and never recorded belongs to
    /// nobody and still has to go.
    #[tokio::test]
    async fn failure_drive_loser_deletes_its_file_even_when_the_winner_will_retry() {
        let (scratch, accounts, _provider, clock, recorder) = harness("failure-both");
        let recording = transferring(&accounts, &recorder, &clock).await;

        let delivery = FakeDelivery::default();
        *delivery.preempt.lock().unwrap() = Some(scratch.path().to_path_buf());
        *delivery.preempt_sql.lock().unwrap() = Some(
            "UPDATE recordings SET drive_file_id = 'drive-winner', error = 'drive_failed' WHERE state = 'transferring'"
                .to_string(),
        );

        assert_eq!(
            deliver_recording(
                &accounts,
                &recorder,
                &delivery,
                &recording,
                "one@example.test",
                86_400,
            )
            .await,
            Err(Failure::Drive)
        );
        let cleanup = delivery
            .calls()
            .into_iter()
            .find(|call| call.starts_with("revoke_and_delete"))
            .expect("an uploaded file nothing names has to go");
        assert!(cleanup.contains("file=drive-file-1"), "{cleanup}");
        assert!(
            cleanup.contains("object=-"),
            "the retry needs the staged bytes: {cleanup}"
        );
    }

    #[tokio::test]
    async fn failure_drive_delivers_when_it_can() {
        let (scratch, accounts, _provider, clock, recorder) = harness("delivery");
        let recording = transferring(&accounts, &recorder, &clock).await;
        let delivery = FakeDelivery::default();

        // Read off the clock the delivery itself reads, because the deadline is
        // twenty-four hours from the share rather than from whenever the caller
        // decided to start one.
        let expires_at = clock.now() + 86_400;

        assert_eq!(
            deliver_recording(
                &accounts,
                &recorder,
                &delivery,
                &recording,
                "one@example.test",
                86_400,
            )
            .await,
            Ok(RecordingState::Ready)
        );
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Ready);

        let (permission, expiry): (Option<String>, Option<i64>) = scratch
            .open()
            .query_row(
                "SELECT drive_permission_id, expires_at FROM recordings WHERE id = 'rec-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(permission.as_deref(), Some("permission-1"));
        assert_eq!(
            expiry,
            Some(expires_at),
            "retention has a deadline or it is not retention"
        );
    }

    /// Retention: the media, and everything that describes it, going away.
    mod retention {
        use std::sync::Arc;

        use codetrial::recording::{
            Clock, RecordingState, delete_recording, due_for_deletion, recording_by_id,
            sweep_recordings,
        };

        use super::super::lifecycle::{harness, state_of};
        use super::{FakeDelivery, transferring};

        /// A delivered recording, ready and dated.
        async fn delivered(
            accounts: &Arc<codetrial::accounts::Accounts>,
            recorder: &codetrial::recording::Recorder,
            clock: &super::super::lifecycle::TestClock,
            delivery: &FakeDelivery,
        ) -> codetrial::recording::Recording {
            let recording = transferring(accounts, recorder, clock).await;
            codetrial::recording::deliver_recording(
                accounts,
                recorder,
                delivery,
                &recording,
                "one@example.test",
                86_400,
            )
            .await
            .unwrap();
            recording_by_id(accounts, &recording.id).unwrap().unwrap()
        }

        #[tokio::test]
        async fn retention_revokes_and_deletes() {
            let (scratch, accounts, _provider, clock, recorder) = harness("retention-deletes");
            let delivery = Arc::new(FakeDelivery::default());
            let ready = delivered(&accounts, &recorder, &clock, &delivery).await;

            // The replay is part of what was recorded, so it is part of what is
            // deleted.
            scratch
                .open()
                .execute(
                    "
        INSERT INTO replay_events (interview_id, seq, kind, at, payload, bytes, received_at)
        VALUES ('int-1', 0, 'transcript', 1, '{}', 2, 1)
        ",
                    [],
                )
                .unwrap();

            // Nothing is due until the deadline passes.
            assert!(due_for_deletion(&accounts, clock.now()).unwrap().is_empty());
            clock.advance(86_401);
            assert_eq!(
                due_for_deletion(&accounts, clock.now())
                    .unwrap()
                    .iter()
                    .map(|row| row.id.clone())
                    .collect::<Vec<_>>(),
                vec![ready.id.clone()]
            );

            let recorder = codetrial::recording::Recorder {
                delivery: Some(delivery.clone()),
                ..recorder
            };
            let outcome = sweep_recordings(&accounts, &recorder).await;
            assert_eq!(outcome.deleted, 1);
            assert_eq!(state_of(&accounts, &ready.id), RecordingState::Deleted);

            let calls = delivery.calls();
            assert_eq!(
                calls[calls.len() - 3..],
                [
                    "revoke on=drive-file-1 permission=permission-1".to_string(),
                    "delete_file file=drive-file-1".to_string(),
                    "delete_object object=codetrial/rec-1.mp4".to_string(),
                ],
                "revoke, file, object, in that order: {calls:?}"
            );
            let replay: i64 = scratch
                .open()
                .query_row(
                    "SELECT COUNT(*) FROM replay_events WHERE interview_id = 'int-1'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                replay, 0,
                "a replay kept after the media is gone keeps the interview"
            );

            // And a second pass finds nothing: a tombstoned row is not due
            // again.
            assert!(due_for_deletion(&accounts, clock.now()).unwrap().is_empty());
        }

        #[tokio::test]
        async fn retention_waits_for_a_job_that_never_stopped() {
            // Tombstoning gives up the room name, which is the only thing the
            // orphan sweep has to stop an Egress job with. A recording deleted
            // while its job is still running is a job nobody can stop, writing
            // media into a bucket this pass has just emptied.
            let (scratch, accounts, provider, clock, recorder) = harness("retention-unstopped");
            let delivery = Arc::new(FakeDelivery::default());
            let ready = delivered(&accounts, &recorder, &clock, &delivery).await;
            scratch
                .open()
                .execute(
                    "UPDATE recordings SET stopped_at = NULL, expires_at = 1 WHERE id = ?1",
                    [&ready.id],
                )
                .unwrap();

            assert!(
                due_for_deletion(&accounts, clock.now()).unwrap().is_empty(),
                "not while the job may still be writing"
            );

            // The orphan sweep stops it, and then it is due.
            let recorder = codetrial::recording::Recorder {
                delivery: Some(delivery.clone()),
                ..recorder
            };
            clock.advance(codetrial::recording::STALE_SECONDS + 1);
            sweep_recordings(&accounts, &recorder).await;
            assert!(!provider.stops().is_empty(), "the job is stopped first");
            assert_eq!(
                due_for_deletion(&accounts, clock.now()).unwrap().len(),
                1,
                "and then the media can go"
            );
        }

        #[tokio::test]
        async fn retention_partial_failure_retries() {
            let (scratch, accounts, _provider, clock, recorder) = harness("retention-partial");
            let delivery = Arc::new(FakeDelivery::default());
            let ready = delivered(&accounts, &recorder, &clock, &delivery).await;
            clock.advance(86_401);

            // The staged object refuses. The revoke and the file deletion
            // already happened, and the row says so.
            *delivery.refuse_object_delete.lock().unwrap() = true;
            assert!(
                delete_recording(&accounts, &recorder, delivery.as_ref(), &ready, "expiry")
                    .await
                    .is_err()
            );
            assert_eq!(
                state_of(&accounts, &ready.id),
                RecordingState::CleanupFailed
            );
            let (file, permission, object): (Option<String>, Option<String>, Option<String>) =
                scratch
                    .open()
                    .query_row(
                        "SELECT drive_file_id, drive_permission_id, gcs_object FROM recordings WHERE id = ?1",
                        [&ready.id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .unwrap();
            assert_eq!((file, permission), (None, None), "the steps that worked");
            assert_eq!(
                object.as_deref(),
                Some("codetrial/rec-1.mp4"),
                "and the one that did not"
            );

            // The retry resumes rather than repeating: no second revoke on a
            // permission that is already gone, which is what fails forever.
            *delivery.refuse_object_delete.lock().unwrap() = false;
            let again = recording_by_id(&accounts, &ready.id).unwrap().unwrap();
            let before = delivery.calls().len();
            assert!(
                delete_recording(&accounts, &recorder, delivery.as_ref(), &again, "expiry")
                    .await
                    .is_ok()
            );
            assert_eq!(
                delivery.calls()[before..],
                ["delete_object object=codetrial/rec-1.mp4".to_string()],
                "one step left, and only that step"
            );
            assert_eq!(state_of(&accounts, &ready.id), RecordingState::Deleted);
        }

        #[tokio::test]
        async fn retention_cleanup_script_lists_without_deleting() {
            // `scripts/test.sh` runs `sh -n` over the scripts, which proves
            // they parse. This is the part that matters about this one: what it
            // selects, and that its default mode writes nothing.
            if std::process::Command::new("sh")
                .args(["-c", "command -v sqlite3"])
                .output()
                .map(|out| !out.status.success())
                .unwrap_or(true)
            {
                eprintln!("skipping: sqlite3 is not installed");
                return;
            }

            let (scratch, accounts, _provider, clock, recorder) = harness("retention-script");
            let delivery = Arc::new(FakeDelivery::default());
            let ready = delivered(&accounts, &recorder, &clock, &delivery).await;
            scratch
                .open()
                .execute(
                    "UPDATE recordings SET expires_at = 1 WHERE id = ?1",
                    [&ready.id],
                )
                .unwrap();

            let listed = std::process::Command::new("./scripts/recording-cleanup.sh")
                .args([
                    "--db",
                    scratch.path().to_str().unwrap(),
                    "--now",
                    "1000",
                    "--dry-run",
                ])
                .output()
                .unwrap();
            assert!(listed.status.success(), "{listed:?}");
            assert_eq!(
                String::from_utf8_lossy(&listed.stdout).trim(),
                ready.id,
                "the expired recording, by id"
            );
            assert!(
                String::from_utf8_lossy(&listed.stderr).contains("nothing was changed"),
                "the default mode says what it did not do"
            );

            // And it did not: the row is untouched, media and all.
            let (state, expires_at, file): (String, Option<i64>, Option<String>) = scratch
                .open()
                .query_row(
                    "SELECT state, expires_at, drive_file_id FROM recordings WHERE id = ?1",
                    [&ready.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!((state.as_str(), expires_at), ("ready", Some(1)));
            assert_eq!(file.as_deref(), Some("drive-file-1"));
            assert!(delivery.calls().iter().all(|call| !call.contains("delete")));

            // `--expire` is the operator's, and it says so on the row: the
            // tombstone records a person asking rather than a deadline
            // arriving.
            let marked = std::process::Command::new("./scripts/recording-cleanup.sh")
                .args([
                    "--db",
                    scratch.path().to_str().unwrap(),
                    "--now",
                    "1000",
                    "--expire",
                    &ready.id,
                ])
                .output()
                .unwrap();
            assert!(marked.status.success(), "{marked:?}");
            let asked: Option<String> = scratch
                .open()
                .query_row(
                    "SELECT deleted_by FROM recordings WHERE id = ?1",
                    [&ready.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(asked.as_deref(), Some("operator"));

            let recorder = codetrial::recording::Recorder {
                delivery: Some(delivery.clone()),
                ..recorder
            };
            assert_eq!(sweep_recordings(&accounts, &recorder).await.deleted, 1);
            let tombstone: Option<String> = scratch
                .open()
                .query_row(
                    "SELECT deleted_by FROM recordings WHERE id = ?1",
                    [&ready.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                tombstone.as_deref(),
                Some("operator"),
                "\"expiry\" would say the deadline came, and it did not"
            );

            // An id nobody has is a failure rather than a quiet success.
            let missing = std::process::Command::new("./scripts/recording-cleanup.sh")
                .args([
                    "--db",
                    scratch.path().to_str().unwrap(),
                    "--expire",
                    "rec-nobody",
                ])
                .output()
                .unwrap();
            assert!(!missing.status.success());
        }

        #[tokio::test]
        async fn retention_tombstone_fields() {
            let (scratch, accounts, _provider, clock, recorder) = harness("retention-tombstone");
            let delivery = Arc::new(FakeDelivery::default());
            let ready = delivered(&accounts, &recorder, &clock, &delivery).await;
            clock.advance(86_401);
            delete_recording(&accounts, &recorder, delivery.as_ref(), &ready, "expiry")
                .await
                .unwrap();

            // Read as strings, so one query answers "what is kept" and "what is
            // given up" without a tuple nobody can read.
            let kept: Vec<Option<String>> = scratch
                .open()
                .query_row(
                    "
        SELECT CAST(deleted_at AS TEXT), deleted_by, delete_error,
               recipient_email, room_name, gcs_object
        FROM recordings WHERE id = ?1
        ",
                    [&ready.id],
                    |row| (0..6).map(|column| row.get(column)).collect(),
                )
                .unwrap();
            assert_eq!(
                kept[0].as_deref(),
                Some(clock.now().to_string().as_str()),
                "when"
            );
            assert_eq!(kept[1].as_deref(), Some("expiry"), "and why");
            assert_eq!(kept[2], None, "nothing partial about this one");
            assert_eq!(
                &kept[3..],
                &[None, None, None],
                "an address kept forever against a file that is gone is the opposite of retention"
            );
        }

        #[tokio::test]
        async fn retention_takes_a_withdrawn_recording_without_waiting() {
            // A withdrawal has no deadline: nothing set one, because the
            // recording may never have been delivered. The person said stop,
            // which is the whole schedule.
            let (_scratch, accounts, _provider, clock, recorder) = harness("retention-withdrawn");
            let delivery = Arc::new(FakeDelivery::default());
            let ready = delivered(&accounts, &recorder, &clock, &delivery).await;

            // Withdrawn after delivery, which is the case the transition table
            // will not carry: a `ready` row cannot become `failed`, so the
            // interview is the only place that says stop.
            codetrial::accounts::withdraw_consent(&accounts, "int-1", 1, clock.now()).unwrap();

            let due = due_for_deletion(&accounts, clock.now()).unwrap();
            assert_eq!(due.len(), 1, "no deadline to wait for");

            let recorder = codetrial::recording::Recorder {
                delivery: Some(delivery.clone()),
                ..recorder
            };
            assert_eq!(sweep_recordings(&accounts, &recorder).await.deleted, 1);
            let row = recording_by_id(&accounts, &ready.id).unwrap().unwrap();
            assert_eq!(row.state, RecordingState::Deleted);
        }
    }

    #[tokio::test]
    async fn cleanup_failure() {
        let (scratch, accounts, _provider, clock, recorder) = harness("cleanup");
        let recording = transferring(&accounts, &recorder, &clock).await;
        let delivery = FakeDelivery::default();
        deliver_recording(
            &accounts,
            &recorder,
            &delivery,
            &recording,
            "one@example.test",
            86_400,
        )
        .await
        .unwrap();
        let ready = recording_by_id(&accounts, "rec-1").unwrap().unwrap();

        *delivery.refuse_delete.lock().unwrap() = true;
        assert!(
            delete_recording(&accounts, &recorder, &delivery, &ready, "expiry")
                .await
                .is_err()
        );

        // Three steps in order, and the third never runs because the second
        // refused. Each one is written down as it succeeds, so what stops here
        // is where the next attempt starts rather than where this one began.
        let calls = delivery.calls();
        assert_eq!(
            calls[calls.len() - 2..],
            [
                "revoke on=drive-file-1 permission=permission-1".to_string(),
                "delete_file file=drive-file-1".to_string(),
            ],
            "deletion is revoke, file, object, in that order: {calls:?}"
        );
        let permission: Option<String> = scratch
            .open()
            .query_row(
                "SELECT drive_permission_id FROM recordings WHERE id = 'rec-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            permission, None,
            "the revoke that worked is not repeated by the retry"
        );

        // Terminal for the pipeline and an alert for a person: the media
        // outlived the attempt to delete it, and nothing automatic is going to
        // fix that.
        assert_eq!(state_of(&accounts, "rec-1"), RecordingState::CleanupFailed);
        let email: Option<String> = scratch
            .open()
            .query_row(
                "SELECT recipient_email FROM recordings WHERE id = 'rec-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            email.as_deref(),
            Some("one@example.test"),
            "nothing is tombstoned while the file it describes is still there"
        );

        // And when it does delete, the row gives up everything that described
        // what was deleted.
        *delivery.refuse_delete.lock().unwrap() = false;
        let failed = recording_by_id(&accounts, "rec-1").unwrap().unwrap();
        assert_eq!(
            delete_recording(&accounts, &recorder, &delivery, &failed, "expiry").await,
            Ok(RecordingState::Deleted)
        );
        let (email, drive, by): (Option<String>, Option<String>, Option<String>) = scratch
            .open()
            .query_row(
                "SELECT recipient_email, drive_file_id, deleted_by FROM recordings WHERE id = 'rec-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(email, None);
        assert_eq!(drive, None);
        assert_eq!(by.as_deref(), Some("expiry"));
    }

    /// The audit line is one line of JSON, whatever the provider put in the
    /// error it reported.
    ///
    /// The first version escaped quotes and backslashes by hand, which lets a
    /// newline through: everything after it reads as a second log entry that
    /// nobody wrote.
    #[test]
    fn audit_lines_survive_what_a_provider_can_say() {
        for hostile in [
            "line one\nline two",
            "{\"event\":\"forged\"}",
            "tab\there",
            "quote\" and backslash\\",
            "null\u{0}byte",
        ] {
            // The line `audit` actually emitted, not one rebuilt here: a second
            // copy of the escaping would agree with the first by construction.
            let line =
                codetrial::recording::audit("recording_failed", "rec-1", &[("error", hostile)]);
            assert_eq!(
                line.lines().count(),
                1,
                "one event is one line, whatever is in it: {line}"
            );
            let parsed: serde_json::Value = serde_json::from_str(&line)
                .unwrap_or_else(|error| panic!("{line} should be JSON: {error}"));
            assert_eq!(parsed["event"], "recording_failed");
            assert_eq!(parsed["recording_id"], "rec-1");
            assert_eq!(parsed["error"], hostile);
        }

        // Bounded, because the value can be a provider's error body and a log
        // line is not a place to put one.
        let long = "x".repeat(10_000);
        let line = codetrial::recording::audit("recording_failed", "rec-1", &[("error", &long)]);
        let parsed: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(parsed["error"].as_str().unwrap().len() < 1_000);
    }

    fn _unused(_: &Scratch) {}
}

/// The replay schema: one envelope, six kinds, and everything a replay must not
/// carry.
mod replay {
    use codetrial::recording::{
        MAX_REPLAY_BYTES, MAX_REPLAY_EVENT_BYTES, MAX_REPLAY_EVENTS, MAX_REPLAY_STRING,
        REPLAY_VERSION, ReplayKind, ReplayRejection, parse_replay_event, redact_replay_payload,
    };
    use serde_json::{Value, json};

    fn envelope(kind: &str, payload: Value) -> Value {
        json!({ "v": REPLAY_VERSION, "kind": kind, "at": 1_770_000_000_000i64, "payload": payload })
    }

    /// Ingest, against a real database.
    ///
    /// The sequence is the whole ordering guarantee, so what these check is
    /// that it never repeats and never skips, whatever arrives.
    mod ingest {
        use std::sync::Arc;

        use codetrial::accounts::{Accounts, GitHubLoginConfig};
        use codetrial::recording::{
            Ingest, MAX_REPLAY_EVENT_BYTES, MAX_REPLAY_EVENTS, MAX_REPLAY_STRING, ReplayKind,
            ReplayRejection, SnapshotView, append_replay_events, parse_replay_event, replay_events,
            replay_snapshot,
        };
        use serde_json::json;

        use crate::Scratch;

        fn harness(label: &str) -> (Scratch, Arc<Accounts>) {
            let scratch = Scratch::new(label);
            codetrial::accounts::initialize_account_database(scratch.path()).unwrap();
            {
                let connection = scratch.open();
                crate::seed(&connection);
                connection
                    .execute_batch(
                        "INSERT INTO users (id, github_id, login, email, email_verified, created_at, updated_at)
                           VALUES (2, 202, 'two', 'two@example.test', 1, 1, 1);
                         INSERT INTO interviews (id, account_id, consent_version, consent_at, room_name)
                           VALUES ('int-other', 2, '2026-08-21', 11, 'interview-def67890');",
                    )
                    .unwrap();
            }
            let accounts = Arc::new(
                Accounts::open(GitHubLoginConfig {
                    oauth: None,
                    session_secret: "replay".to_string(),
                    db_path: scratch.path().to_path_buf(),
                    oauth_base_url: String::new(),
                    api_base_url: String::new(),
                })
                .unwrap(),
            );
            (scratch, accounts)
        }

        fn event(kind: ReplayKind, text: &str) -> codetrial::recording::ReplayEvent {
            parse_replay_event(&json!({
                "v": codetrial::recording::REPLAY_VERSION,
                "kind": kind.as_str(),
                "at": 1_770_000_000_000i64,
                "payload": { "text": text }
            }))
            .unwrap()
        }

        #[tokio::test]
        async fn replay_ingest_monotonic() {
            let (_scratch, accounts) = harness("ingest-monotonic");

            assert_eq!(
                append_replay_events(
                    &accounts,
                    "int-1",
                    1,
                    &[
                        event(ReplayKind::Transcript, "one"),
                        event(ReplayKind::Editor, "two")
                    ],
                    10,
                )
                .unwrap(),
                Ingest::Stored { first: 0, last: 1 }
            );
            assert_eq!(
                append_replay_events(
                    &accounts,
                    "int-1",
                    1,
                    &[event(ReplayKind::Tests, "three")],
                    11
                )
                .unwrap(),
                Ingest::Stored { first: 2, last: 2 },
                "a second batch continues where the first stopped"
            );

            let stored = replay_events(&accounts, "int-1", 1, -1).unwrap();
            assert_eq!(
                stored.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(),
                vec![0, 1, 2],
                "no gaps and no repeats, which is the whole ordering guarantee"
            );
            assert_eq!(stored[0].1.payload["text"], "one");

            // Each interview counts on its own, or one candidate's replay would
            // renumber another's.
            assert_eq!(
                append_replay_events(
                    &accounts,
                    "int-other",
                    2,
                    &[event(ReplayKind::Transcript, "theirs")],
                    12,
                )
                .unwrap(),
                Ingest::Stored { first: 0, last: 0 }
            );

            // And somebody else's interview is not this account's to write to.
            assert_eq!(
                append_replay_events(
                    &accounts,
                    "int-other",
                    1,
                    &[event(ReplayKind::Transcript, "not mine")],
                    13,
                )
                .unwrap(),
                Ingest::NoInterview
            );
            assert_eq!(
                replay_events(&accounts, "int-other", 2, -1).unwrap().len(),
                1
            );
        }

        #[tokio::test]
        async fn replay_snapshot_reconstructs() {
            let (_scratch, accounts) = harness("snapshot-reconstructs");

            // Two editor snapshots and two transcript lines. The first editor
            // snapshot is superseded; neither transcript line is, because a
            // transcript accumulates.
            append_replay_events(
                &accounts,
                "int-1",
                1,
                &[
                    event(ReplayKind::Editor, "first draft"),
                    event(ReplayKind::Transcript, "hello"),
                    event(ReplayKind::Editor, "second draft"),
                    event(ReplayKind::Transcript, "how are you"),
                ],
                10,
            )
            .unwrap();

            let SnapshotView::Ready(snapshot) =
                replay_snapshot(&accounts, "int-1", 1, 100).unwrap()
            else {
                panic!("the owner reads their own replay");
            };
            assert_eq!(snapshot.seq, 3);
            assert!(!snapshot.quota_exceeded);
            assert_eq!(
                snapshot
                    .events
                    .iter()
                    .map(|(seq, event)| (*seq, event.kind, event.payload["text"].as_str().unwrap()))
                    .collect::<Vec<_>>(),
                vec![
                    (1, ReplayKind::Transcript, "hello"),
                    (2, ReplayKind::Editor, "second draft"),
                    (3, ReplayKind::Transcript, "how are you"),
                ],
                "the superseded editor snapshot is dropped and nothing else is"
            );

            // A late join carries on from `seq`, and what happens next is only
            // in the events.
            append_replay_events(
                &accounts,
                "int-1",
                1,
                &[event(ReplayKind::Transcript, "fine thanks")],
                11,
            )
            .unwrap();
            let after = replay_events(&accounts, "int-1", 1, snapshot.seq).unwrap();
            assert_eq!(
                after
                    .iter()
                    .map(|(seq, event)| (*seq, event.payload["text"].as_str().unwrap()))
                    .collect::<Vec<_>>(),
                vec![(4, "fine thanks")],
                "no overlap with the snapshot and no gap after it"
            );
        }

        #[tokio::test]
        async fn replay_snapshot_cross_account_denied() {
            let (_scratch, accounts) = harness("snapshot-cross-account");
            append_replay_events(
                &accounts,
                "int-other",
                2,
                &[event(ReplayKind::Transcript, "theirs")],
                10,
            )
            .unwrap();

            assert_eq!(
                replay_snapshot(&accounts, "int-other", 1, 100).unwrap(),
                SnapshotView::NoInterview,
                "an account that does not own an interview does not learn it exists"
            );
            assert!(matches!(
                replay_snapshot(&accounts, "int-other", 2, 100).unwrap(),
                SnapshotView::Ready(_)
            ));

            // Withdrawal closes the replay in both directions. A replay nobody
            // may add to is not one to keep handing out either.
            codetrial::accounts::withdraw_consent(&accounts, "int-other", 2, 50).unwrap();
            assert_eq!(
                replay_snapshot(&accounts, "int-other", 2, 100).unwrap(),
                SnapshotView::NoInterview
            );
        }

        #[tokio::test]
        async fn replay_snapshot_expired() {
            let (scratch, accounts) = harness("snapshot-expired");
            append_replay_events(
                &accounts,
                "int-1",
                1,
                &[event(ReplayKind::Transcript, "mine")],
                10,
            )
            .unwrap();
            scratch
                .open()
                .execute(
                    "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, expires_at, created_at, updated_at
        ) VALUES ('rec-1', 1, 'int-1', 'interview-abc12345', 'int-1',
            'one@example.test', 'ready', 100, 1, 1)
        ",
                    [],
                )
                .unwrap();

            assert!(matches!(
                replay_snapshot(&accounts, "int-1", 1, 99).unwrap(),
                SnapshotView::Ready(_)
            ));
            assert_eq!(
                replay_snapshot(&accounts, "int-1", 1, 100).unwrap(),
                SnapshotView::Expired,
                "the deadline is the deadline, whether or not the sweeper has run"
            );

            // Deleted is its own answer, because the media is gone rather than
            // out of reach.
            scratch
                .open()
                .execute(
                    "
        UPDATE recordings SET state = 'deleted', deleted_at = 90, expires_at = NULL,
            room_name = NULL, recipient_email = NULL WHERE id = 'rec-1'
        ",
                    [],
                )
                .unwrap();
            assert_eq!(
                replay_snapshot(&accounts, "int-1", 1, 50).unwrap(),
                SnapshotView::Deleted
            );
        }

        #[tokio::test]
        async fn replay_ingest_concurrent_writers() {
            let (scratch, accounts) = harness("ingest-concurrent");

            // Separate handles, so these are separate SQLite connections and
            // not two callers behind one mutex. Four writers, five batches
            // each, two events per batch: enough interleaving that a
            // SELECT-then-INSERT allocation loses a race almost every run.
            let writers: Vec<_> = (0..4)
                .map(|_| {
                    let accounts = Arc::new(
                        Accounts::open(GitHubLoginConfig {
                            oauth: None,
                            session_secret: "replay".to_string(),
                            db_path: scratch.path().to_path_buf(),
                            oauth_base_url: String::new(),
                            api_base_url: String::new(),
                        })
                        .unwrap(),
                    );
                    std::thread::spawn(move || {
                        for round in 0..5 {
                            append_replay_events(
                                &accounts,
                                "int-1",
                                1,
                                &[
                                    event(ReplayKind::Transcript, "a"),
                                    event(ReplayKind::Editor, "b"),
                                ],
                                round,
                            )
                            .unwrap();
                        }
                    })
                })
                .collect();
            for writer in writers {
                writer.join().unwrap();
            }

            let stored = replay_events(&accounts, "int-1", 1, -1).unwrap();
            assert_eq!(
                stored.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(),
                (0..40).collect::<Vec<_>>(),
                "forty events, numbered once each, with nothing skipped"
            );
            drop(accounts);
        }

        #[tokio::test]
        async fn replay_ingest_oversize_event() {
            // Refused at the parse, before anything reaches the database: the
            // ingest path never sees an event the schema would not have.
            //
            // Built from many fields rather than one enormous string, because a
            // single string is trimmed to `MAX_REPLAY_STRING` and would come
            // back under the limit: what the event ceiling catches is bulk the
            // trim cannot reach.
            let payload: serde_json::Map<String, serde_json::Value> = (0..8)
                .map(|field| {
                    (
                        format!("f{field}"),
                        serde_json::Value::String("q".repeat(MAX_REPLAY_STRING)),
                    )
                })
                .collect();
            let oversize = json!({
                "v": codetrial::recording::REPLAY_VERSION,
                "kind": "editor",
                "at": 1,
                "payload": payload
            });
            assert!(
                codetrial::recording::payload_bytes(&oversize["payload"]) > MAX_REPLAY_EVENT_BYTES
            );
            assert_eq!(
                parse_replay_event(&oversize),
                Err(ReplayRejection::Oversize)
            );

            let (_scratch, accounts) = harness("ingest-oversize");
            assert_eq!(replay_events(&accounts, "int-1", 1, -1).unwrap().len(), 0);
        }

        #[tokio::test]
        async fn replay_ingest_quota_refusal() {
            let (scratch, accounts) = harness("ingest-quota");
            scratch
                .open()
                .execute(
                    "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        ) VALUES ('rec-1', 1, 'int-1', 'interview-abc12345', 'int-1',
            'one@example.test', 'recording', 1, 1)
        ",
                    [],
                )
                .unwrap();

            // Fill to the event ceiling directly, because writing five thousand
            // events through the ingest path tests SQLite rather than the
            // quota.
            let connection = scratch.open();
            for seq in 0..MAX_REPLAY_EVENTS {
                connection
                    .execute(
                        "
        INSERT INTO replay_events (interview_id, seq, kind, at, payload, bytes, received_at)
        VALUES ('int-1', ?1, 'transcript', 1, '{}', 2, 1)
        ",
                        [seq],
                    )
                    .unwrap();
            }

            assert_eq!(
                append_replay_events(
                    &accounts,
                    "int-1",
                    1,
                    &[event(ReplayKind::Transcript, "x")],
                    20
                )
                .unwrap(),
                Ingest::QuotaExceeded
            );
            assert_eq!(
                replay_events(&accounts, "int-1", 1, -1).unwrap().len() as i64,
                MAX_REPLAY_EVENTS,
                "nothing is stored past the ceiling"
            );

            // The recording is flagged and not failed. A replay that stopped
            // growing is still a recording worth keeping, and the flag is where
            // a reviewer sees that its tail is missing.
            let (state, flag): (String, i64) = scratch
                .open()
                .query_row(
                    "SELECT state, quota_exceeded FROM recordings WHERE id = 'rec-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(state, "recording");
            assert_eq!(flag, 1);
        }
    }

    #[test]
    fn replay_schema_envelope() {
        // One shape for six producers. A renderer reads `kind` to decide what
        // to draw, and an envelope it cannot read is an event it cannot place.
        // Written out, not iterated from `ALL`: `parse` reads `ALL` too, so a
        // loop over it would agree with itself whatever the list said.
        assert_eq!(
            ReplayKind::ALL.map(ReplayKind::as_str),
            [
                "transcript",
                "editor",
                "tests",
                "stage",
                "avatar",
                "lifecycle"
            ]
        );

        for kind in ReplayKind::ALL {
            let parsed = parse_replay_event(&envelope(kind.as_str(), json!({ "a": 1 })))
                .unwrap_or_else(|_| panic!("{} should parse", kind.as_str()));
            assert_eq!(parsed.kind, kind);
            assert_eq!(parsed.at, 1_770_000_000_000);
        }

        // The version is checked, not assumed. A producer from a later deploy
        // sending a shape this server does not know is refused rather than
        // stored and rendered as something else.
        let future =
            json!({ "v": REPLAY_VERSION + 1, "kind": "transcript", "at": 1, "payload": {} });
        assert_eq!(parse_replay_event(&future), Err(ReplayRejection::Envelope));

        assert_eq!(
            parse_replay_event(&envelope("screenshot", json!({}))),
            Err(ReplayRejection::Kind),
            "a kind nothing renders is a producer nobody wrote a renderer for"
        );
        for broken in [
            json!({ "kind": "transcript", "at": 1, "payload": {} }),
            json!({ "v": REPLAY_VERSION, "at": 1, "payload": {} }),
            json!({ "v": REPLAY_VERSION, "kind": "transcript", "payload": {} }),
            json!({ "v": REPLAY_VERSION, "kind": "transcript", "at": -1, "payload": {} }),
            json!({ "v": REPLAY_VERSION, "kind": "transcript", "at": 1, "payload": "text" }),
        ] {
            assert_eq!(parse_replay_event(&broken), Err(ReplayRejection::Envelope));
        }

        // Which kinds replace their predecessor is what makes a late join one
        // snapshot plus the events after it, rather than every keystroke.
        for replaces in [
            ReplayKind::Editor,
            ReplayKind::Stage,
            ReplayKind::Avatar,
            ReplayKind::Lifecycle,
        ] {
            assert!(replaces.is_snapshot(), "{}", replaces.as_str());
        }
        for accumulates in [ReplayKind::Transcript, ReplayKind::Tests] {
            assert!(!accumulates.is_snapshot(), "{}", accumulates.as_str());
        }
    }

    #[test]
    fn replay_schema_limits() {
        assert_eq!(MAX_REPLAY_EVENT_BYTES, 64 * 1024);
        assert_eq!(MAX_REPLAY_EVENTS, 5_000);
        assert_eq!(MAX_REPLAY_BYTES, 8 * 1024 * 1024);

        // A code snapshot the size of a screen goes through.
        let code = "x".repeat(4_000);
        assert!(parse_replay_event(&envelope("editor", json!({ "code": code }))).is_ok());

        // Bytes, not characters. Sixteen thousand four-byte characters are the
        // whole event budget spent on one field, and the cut lands on a
        // character boundary rather than splitting one.
        let wide = "\u{1F600}".repeat(MAX_REPLAY_STRING / 4 + 10);
        let cut = parse_replay_event(&envelope("editor", json!({ "code": wide })))
            .unwrap()
            .payload["code"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(cut.len() <= MAX_REPLAY_STRING);
        assert!(cut.chars().all(|character| character == '\u{1F600}'));

        // One string longer than a producer has any reason to send is cut, and
        // the event survives: the interview is worth more than the tail of one
        // oversized value.
        let long = "y".repeat(MAX_REPLAY_STRING + 500);
        let parsed = parse_replay_event(&envelope("editor", json!({ "code": long }))).unwrap();
        assert_eq!(
            parsed.payload["code"].as_str().unwrap().chars().count(),
            MAX_REPLAY_STRING
        );

        // Many of them together is not an event at all.
        let fields: serde_json::Map<String, Value> = (0..8)
            .map(|index| (format!("f{index}"), json!("z".repeat(MAX_REPLAY_STRING))))
            .collect();
        assert_eq!(
            parse_replay_event(&envelope("editor", Value::Object(fields))),
            Err(ReplayRejection::Oversize)
        );

        // One overlong string is cut and the event kept, which is the promise
        // the contract makes about a code snapshot at the limit.
        let huge = "q".repeat(MAX_REPLAY_EVENT_BYTES + 10_000);
        let kept = parse_replay_event(&envelope("editor", json!({ "code": huge }))).unwrap();
        assert_eq!(
            kept.payload["code"].as_str().unwrap().len(),
            MAX_REPLAY_STRING
        );

        // Bulk hidden under keys redaction removes is still bulk. Stripping
        // runs after the measurement for exactly this: removal is not something
        // a producer may rely on to get under the limit.
        let hidden: serde_json::Map<String, Value> = (0..8)
            .map(|index| {
                (
                    format!("accessToken{index}"),
                    json!("q".repeat(MAX_REPLAY_STRING)),
                )
            })
            .collect();
        assert_eq!(
            parse_replay_event(&envelope("editor", Value::Object(hidden))),
            Err(ReplayRejection::Oversize),
            "an oversized event is oversized whether or not its bulk survives redaction"
        );
    }

    #[test]
    fn replay_schema_redaction() {
        // Removed, not refused. A producer that accidentally carried a token
        // should still deliver the transcript line beside it.
        let parsed = parse_replay_event(&envelope(
            "transcript",
            json!({
                "text": "hello",
                "accessToken": "ya29.secret",
                "Authorization": "Bearer nope",
                "x-api-key": "k1",
                "private_key": "k2",
                "sessionId": "abc",
                "nested": { "apiKey": "k", "kept": true }
            }),
        ))
        .unwrap();
        assert_eq!(parsed.payload["text"], "hello");
        assert_eq!(parsed.payload["nested"]["kept"], true);

        // Separators removed before matching, because the browser writes all of
        // these and a list of exact names would let every one through.
        for gone in ["accessToken", "Authorization", "x-api-key", "private_key"] {
            assert!(parsed.payload.get(gone).is_none(), "{gone} should be gone");
        }
        assert!(parsed.payload["nested"].get("apiKey").is_none());

        // And `sessionId` stays. `session` as a marker would take it and
        // `sessionName` with it, and the session cookie is HttpOnly and
        // unreachable from any producer.
        assert_eq!(parsed.payload["sessionId"], "abc");

        // The one thing a replay must never carry. The video is the provider's,
        // delivered under a permission that expires, and a frame smuggled into
        // an event outlives it.
        let media = parse_replay_event(&envelope(
            "avatar",
            json!({
                "frame": "data:image/png;base64,iVBORw0KGgo=",
                "other": "  BLOB:https://example.test/abc",
                "items": ["data:video/mp4;base64,AAAA"]
            }),
        ))
        .unwrap();
        assert_eq!(media.payload["frame"], "[media removed]");
        assert_eq!(media.payload["other"], "[media removed]");
        assert_eq!(media.payload["items"][0], "[media removed]");

        // And the redaction is the same function the ingest path uses, applied
        // to a value that never went through an envelope.
        let direct = redact_replay_payload(&json!({ "refreshToken": "r", "keep": 1 }));
        assert!(direct.get("refreshToken").is_none());
        assert_eq!(direct["keep"], 1);
    }
}

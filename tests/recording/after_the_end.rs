//! A recording that has already ended, and what the provider is and is not
//! asked about it.

use codetrial::recording::{
    Clock, DELIVERY_BACKOFF_SECONDS, Failure, MIN_GRANT_REMAINING_SECONDS, RETENTION_SECONDS,
    RecordingState, delete_recording, deliver_recording, due_for_deletion, enqueue_delivery,
    record_egress_id, recording_by_id, run_delivery_queue, sweep_recordings,
};

use std::sync::Arc;

use codetrial::recording::{BoxFuture, Recorder, RecordingProvider, StartEgress};
use codetrial::recording::{SnapshotView::Expired, recording_summary, replay_snapshot};

use crate::failure::{FakeDelivery, transferring};
use crate::lifecycle::{FakeProvider, harness, start, state_of};

/// The fake provider, with something done while it is asked whether a room
/// already has a job: where a deletion lands between a retry's claim and its
/// write.
struct ActsWhenAsked {
    inner: Arc<FakeProvider>,
    act: Box<dyn Fn() + Send + Sync>,
}

impl RecordingProvider for ActsWhenAsked {
    fn start<'a>(&'a self, request: &'a StartEgress) -> BoxFuture<'a, Result<String, String>> {
        self.inner.start(request)
    }

    fn stop<'a>(
        &'a self,
        egress_id: &'a str,
        room_name: &'a str,
    ) -> BoxFuture<'a, Result<(), String>> {
        self.inner.stop(egress_id, room_name)
    }

    fn active_for_room<'a>(
        &'a self,
        room_name: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, String>> {
        (self.act)();
        self.inner.active_for_room(room_name)
    }
}

/// A recording that already ended reaches no provider at all.
///
/// The staged object is written down first and only for a `transferring`
/// row, so a delivery called after a withdrawal stops before it can make a
/// Drive copy of a recording nobody consented to keep.
#[tokio::test]
async fn failure_drive_refuses_a_recording_that_already_ended() {
    let (scratch, accounts, _provider, _clock, recorder) = harness("failure-ended");
    let recording = transferring(&accounts, &recorder).await;
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

/// A withdrawal before the transfer leaves the staged object unnamed.
///
/// Egress writes the file at a path derived from the recording id from the
/// moment it starts, but the row learns it only when a delivery stages it.
/// Deleting by the column alone would tombstone the row and leave the video
/// in the bucket.
#[tokio::test]
async fn deleting_a_recording_that_never_staged_still_removes_its_object() {
    let (scratch, accounts, _provider, _clock, recorder) = harness("never-staged");
    let recording = transferring(&accounts, &recorder).await;
    scratch
        .open()
        .execute(
            "UPDATE recordings SET state = 'failed', error = 'consent_withdrawn' WHERE id = 'rec-1'",
            [],
        )
        .unwrap();
    let row = recording_by_id(&accounts, &recording.id).unwrap().unwrap();

    let delivery = FakeDelivery::default();
    assert!(
        delete_recording(&accounts, &recorder, &delivery, &row, "consent")
            .await
            .is_ok()
    );
    assert_eq!(
        delivery.calls(),
        ["delete_object object=codetrial/rec-1.mp4".to_string()],
        "the object the egress job writes is deleted by its derived path"
    );
    assert_eq!(state_of(&accounts, "rec-1"), RecordingState::Deleted);
}

/// A job that comes back after the row was tombstoned is not recorded on it.
///
/// A withdrawal can fail a row while its start is still waiting on the
/// provider, and the retention sweep can then tombstone it, which clears the
/// room name. Writing the id on that row would send the settle step to stop the
/// job by an empty room name, so the caller has to be told the job is its own
/// to stop, with the room it started it in.
#[tokio::test]
async fn a_job_that_arrives_after_the_row_was_deleted_is_not_recorded() {
    let (scratch, accounts, _provider, _clock, recorder) = harness("egress-after-delete");
    let recording = start(&accounts, &recorder, "int-1", "rec-1").unwrap();
    scratch
        .open()
        .execute(
            "UPDATE recordings SET state = 'deleted', room_name = NULL, recipient_email = NULL, deleted_at = 60 WHERE id = 'rec-1'",
            [],
        )
        .unwrap();

    assert!(
        !record_egress_id(&accounts, recorder.clock.as_ref(), &recording.id, "EG_late").unwrap(),
        "the caller keeps a job the row does not name"
    );
    let named: Option<String> = scratch
        .open()
        .query_row(
            "SELECT egress_id FROM recordings WHERE id = 'rec-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(named, None);
}

/// A delivery that ran out of attempts is not kept forever.
///
/// Its Drive file, recipient address and replay stay on the row for an
/// operator to retry, but a failed row had no deadline, so nothing would ever
/// delete them. The row is due the retention window after its last failure,
/// which is also how long the operator has to retry, and a retry that fails
/// again starts the window over.
#[tokio::test]
async fn a_delivery_out_of_attempts_is_due_a_retention_window_after_it_failed() {
    let (_scratch, accounts, _provider, clock, recorder) = harness("exhausted-deadline");
    let recording = transferring(&accounts, &recorder).await;
    _scratch
        .open()
        .execute("UPDATE recordings SET expires_at = 1", [])
        .unwrap();
    enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();
    let delivery = FakeDelivery::default();
    *delivery.refuse_transfer.lock().unwrap() = true;
    let exhaust = || async {
        for backoff in DELIVERY_BACKOFF_SECONDS {
            run_delivery_queue(&accounts, &recorder, &delivery).await;
            clock.advance(backoff);
        }
        run_delivery_queue(&accounts, &recorder, &delivery).await;
        assert_eq!(state_of(&accounts, &recording.id), RecordingState::Failed);
        clock.now()
    };
    let failed_at = exhaust().await;
    assert!(due_for_deletion(&accounts, failed_at).unwrap().is_empty());
    assert!(
        due_for_deletion(&accounts, failed_at + RETENTION_SECONDS - 1)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        due_for_deletion(&accounts, failed_at + RETENTION_SECONDS)
            .unwrap()
            .len(),
        1,
        "due once the window an operator has has passed"
    );

    // The operator retries and it fails again: the window starts over.
    clock.advance(RETENTION_SECONDS / 2);
    enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();
    let failed_again = exhaust().await;
    assert!(
        due_for_deletion(&accounts, failed_again + RETENTION_SECONDS - 1)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        due_for_deletion(&accounts, failed_again + RETENTION_SECONDS)
            .unwrap()
            .len(),
        1
    );
}

/// A deletion of an exhausted delivery that fails halfway is tried again.
///
/// The failed attempt moves the row to `cleanup_failed` and replaces its
/// `drive_failed` error, so a clause that matched only that error stopped
/// matching the row, and its Drive file and recipient stayed for good.
#[tokio::test]
async fn an_exhausted_delivery_whose_deletion_failed_is_due_again() {
    let (scratch, accounts, _provider, clock, recorder) = harness("exhausted-cleanup-failed");
    let recording = transferring(&accounts, &recorder).await;
    enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();
    let delivery = FakeDelivery::default();
    *delivery.refuse_transfer.lock().unwrap() = true;
    for backoff in DELIVERY_BACKOFF_SECONDS {
        run_delivery_queue(&accounts, &recorder, &delivery).await;
        clock.advance(backoff);
    }
    run_delivery_queue(&accounts, &recorder, &delivery).await;
    let failed_at = clock.now();

    scratch
        .open()
        .execute(
            "UPDATE recordings SET state = 'cleanup_failed', error = 'cleanup_failed', expires_at = 1
             WHERE id = ?1",
            [&recording.id],
        )
        .unwrap();
    assert_eq!(
        due_for_deletion(&accounts, failed_at + RETENTION_SECONDS)
            .unwrap()
            .len(),
        1,
        "a partial deletion resumes on the next pass"
    );
}

/// A job adopted for a row that was deleted in the meantime is stopped.
///
/// The row no longer names the job and the tombstone gave up the room, so
/// nothing else can find it: the retry is the only one that can stop it.
#[tokio::test]
async fn an_adopted_job_is_stopped_when_its_row_was_deleted() {
    let (scratch, accounts, provider, clock, recorder) = harness("adopt-deleted");
    start(&accounts, &recorder, "int-1", "rec-1").unwrap();
    *provider.adopt.lock().unwrap() = Some("EG_orphan".to_string());
    let path = scratch.path().to_path_buf();
    let recorder = Recorder {
        provider: Arc::new(ActsWhenAsked {
            inner: Arc::clone(&provider),
            act: Box::new(move || {
                rusqlite::Connection::open(&path)
                    .unwrap()
                    .execute(
                        "UPDATE recordings SET state = 'deleted', room_name = NULL, \
                         recipient_email = NULL, deleted_at = 60 WHERE id = 'rec-1'",
                        [],
                    )
                    .unwrap();
            }),
        }),
        clock: Arc::clone(&recorder.clock),
        config: recorder.config.clone(),
        delivery: None,
    };

    clock.advance(60);
    assert_eq!(sweep_recordings(&accounts, &recorder).await.retried, 0);
    let stops = provider.stops();
    assert_eq!(
        stops.len(),
        1,
        "the job nothing names is stopped: {stops:?}"
    );
    assert!(stops[0].starts_with("EG_orphan@"), "{stops:?}");
}
const FAIL_ONCE: &str = "
    CREATE TRIGGER fail_finish BEFORE UPDATE ON recordings
    WHEN NEW.state = 'ready' AND OLD.retries = 0
    BEGIN SELECT RAISE(FAIL, 'finish'); END
";

fn revokes_of(drive: &FakeDelivery) -> usize {
    drive
        .calls()
        .into_iter()
        .filter(|call| call.contains("permission=permission-1"))
        .count()
}

#[tokio::test]
async fn transfer_retry_after_share_keeps_the_drive_expiry() {
    let (scratch, accounts, _, clock, recorder) = harness("retry-keeps-expiry");
    let (db, drive) = (scratch.open(), FakeDelivery::default());
    let recording = transferring(&accounts, &recorder).await;
    db.execute_batch(FAIL_ONCE).unwrap();
    enqueue_delivery(&accounts, &recording.id, 1_000).unwrap();

    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(
        state_of(&accounts, &recording.id),
        RecordingState::Transferring
    );
    assert_eq!(*drive.shared_expires_at.lock().unwrap(), vec![87_400]);

    clock.advance(60);
    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(state_of(&accounts, &recording.id), RecordingState::Ready);
    assert_eq!(*drive.shared_expires_at.lock().unwrap(), vec![87_400]);
    assert_eq!(revokes_of(&drive), 0);
    assert_eq!(
        recording_summary(&accounts, &recording.id, 1)
            .unwrap()
            .unwrap()
            .expires_at,
        Some(87_400)
    );
}

#[tokio::test]
async fn operator_retry_shortly_before_expiry_revokes_and_reshares() {
    let (scratch, accounts, _, clock, recorder) = harness("operator-retry-reshare");
    let (db, drive) = (scratch.open(), FakeDelivery::default());
    let recording = transferring(&accounts, &recorder).await;
    db.execute_batch(FAIL_ONCE).unwrap();
    enqueue_delivery(&accounts, &recording.id, 1_000).unwrap();
    db.execute("UPDATE delivery_queue SET attempts = 4", [])
        .unwrap();

    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(state_of(&accounts, &recording.id), RecordingState::Failed);
    assert_eq!(*drive.shared_expires_at.lock().unwrap(), vec![87_400]);

    clock.advance(86_300);
    enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();
    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(state_of(&accounts, &recording.id), RecordingState::Ready);
    assert_eq!(revokes_of(&drive), 1);
    assert_eq!(
        *drive.shared_expires_at.lock().unwrap(),
        vec![87_400, 173_700]
    );
    assert_eq!(
        recording_summary(&accounts, &recording.id, 1)
            .unwrap()
            .unwrap()
            .expires_at,
        Some(173_700)
    );
}

#[tokio::test]
async fn operator_expired_failed_row_with_queued_retry_stays_due() {
    let (scratch, accounts, _, clock, recorder) = harness("operator-expired-queued");
    let (db, drive) = (scratch.open(), FakeDelivery::default());
    let recording = transferring(&accounts, &recorder).await;
    db.execute_batch(FAIL_ONCE).unwrap();
    enqueue_delivery(&accounts, &recording.id, 1_000).unwrap();
    db.execute("UPDATE delivery_queue SET attempts = 4", [])
        .unwrap();

    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(state_of(&accounts, &recording.id), RecordingState::Failed);

    enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();
    db.execute(
        "UPDATE recordings SET expires_at = ?1, deleted_by = 'operator'",
        [clock.now()],
    )
    .unwrap();

    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(state_of(&accounts, &recording.id), RecordingState::Failed);
    assert_eq!(due_for_deletion(&accounts, clock.now()).unwrap().len(), 1);

    // If `--expire` lands while an active attempt is revoking the old grant,
    // `forget_drive_file` must clear `drive_permission_id` without clearing the
    // operator's `expires_at`, and `share_file` stops without calling `share`
    // once the row is no longer `transferring`.
    enqueue_delivery(&accounts, &recording.id, clock.now()).unwrap();
    db.execute_batch(
        "UPDATE recordings SET state = 'transferring';
         CREATE TRIGGER expire_during_revoke
         AFTER UPDATE OF drive_permission_id ON recordings
         WHEN NEW.drive_permission_id IS NULL
         BEGIN
             UPDATE recordings SET state = 'failed', error = 'drive_failed'
             WHERE id = OLD.id;
         END;",
    )
    .unwrap();
    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(state_of(&accounts, &recording.id), RecordingState::Failed);
    assert_eq!(*drive.shared_expires_at.lock().unwrap(), vec![87_400]);
    assert_eq!(due_for_deletion(&accounts, clock.now()).unwrap().len(), 1);
}

#[tokio::test]
async fn retried_delivery_revokes_and_reshares_an_expired_grant() {
    let (scratch, accounts, _, clock, recorder) = harness("expired-grant-reshare");
    let (db, drive) = (scratch.open(), FakeDelivery::default());
    let recording = transferring(&accounts, &recorder).await;
    db.execute_batch(FAIL_ONCE).unwrap();
    enqueue_delivery(&accounts, &recording.id, 1_000).unwrap();

    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(*drive.shared_expires_at.lock().unwrap(), vec![87_400]);

    // Once the remaining window falls to `MIN_GRANT_REMAINING_SECONDS` (or
    // below it), `share_file` revokes the expiring grant and shares again. When
    // revoking fails, `share_file` stops before calling `share` and keeps the
    // recorded permission for the next attempt.
    *drive.refuse_delete.lock().unwrap() = true;
    clock.advance(RETENTION_SECONDS - MIN_GRANT_REMAINING_SECONDS);
    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(
        state_of(&accounts, &recording.id),
        RecordingState::Transferring
    );
    assert_eq!(*drive.shared_expires_at.lock().unwrap(), vec![87_400]);

    *drive.refuse_delete.lock().unwrap() = false;
    clock.advance(300);
    let expected_expires_at = clock.now() + RETENTION_SECONDS;
    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(state_of(&accounts, &recording.id), RecordingState::Ready);
    assert_eq!(revokes_of(&drive), 2);
    assert_eq!(
        *drive.shared_expires_at.lock().unwrap(),
        vec![87_400, expected_expires_at]
    );
    assert_eq!(
        recording_summary(&accounts, &recording.id, 1)
            .unwrap()
            .unwrap()
            .expires_at,
        Some(expected_expires_at)
    );
}

#[tokio::test]
async fn retried_delivery_revokes_and_reshares_a_grant_with_no_recorded_expiry() {
    let (scratch, accounts, _, clock, recorder) = harness("missing-expiry-final-attempt");
    let (db, drive) = (scratch.open(), FakeDelivery::default());
    let recording = transferring(&accounts, &recorder).await;
    db.execute_batch(FAIL_ONCE).unwrap();
    enqueue_delivery(&accounts, &recording.id, 1_000).unwrap();

    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(*drive.shared_expires_at.lock().unwrap(), vec![87_400]);

    // An old-format row on its final attempt: `drive_permission_id` is set,
    // `expires_at` is NULL, and no further retry attempt remains.
    db.execute_batch(
        "UPDATE recordings SET expires_at = NULL;
         UPDATE delivery_queue SET attempts = 2;",
    )
    .unwrap();
    clock.advance(60);
    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(state_of(&accounts, &recording.id), RecordingState::Ready);
    assert_eq!(revokes_of(&drive), 1);
    assert_eq!(
        *drive.shared_expires_at.lock().unwrap(),
        vec![87_400, 87_460]
    );
    assert_eq!(
        recording_summary(&accounts, &recording.id, 1)
            .unwrap()
            .unwrap()
            .expires_at,
        Some(87_460)
    );
}

#[tokio::test]
async fn grant_expiry_stays_hidden_from_summary_until_operator_expiry() {
    let (scratch, accounts, _, _, recorder) = harness("grant-hidden-from-summary");
    let (db, drive) = (scratch.open(), FakeDelivery::default());
    let recording = transferring(&accounts, &recorder).await;
    db.execute_batch(FAIL_ONCE).unwrap();
    enqueue_delivery(&accounts, &recording.id, 1_000).unwrap();
    db.execute("UPDATE delivery_queue SET attempts = 4", [])
        .unwrap();

    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert_eq!(state_of(&accounts, &recording.id), RecordingState::Failed);
    assert_eq!(
        recording_summary(&accounts, &recording.id, 1)
            .unwrap()
            .unwrap()
            .expires_at,
        None
    );

    db.execute("UPDATE recordings SET deleted_by = 'operator'", [])
        .unwrap();
    assert_eq!(
        recording_summary(&accounts, &recording.id, 1)
            .unwrap()
            .unwrap()
            .expires_at,
        Some(87_400)
    );
}

#[tokio::test]
async fn grant_expiry_does_not_expire_replay_before_delivery() {
    let (scratch, accounts, _, _, recorder) = harness("grant-hidden-from-replay");
    let (db, drive) = (scratch.open(), FakeDelivery::default());
    let recording = transferring(&accounts, &recorder).await;
    db.execute_batch(FAIL_ONCE).unwrap();
    enqueue_delivery(&accounts, &recording.id, 1_000).unwrap();
    db.execute("UPDATE delivery_queue SET attempts = 4", [])
        .unwrap();

    run_delivery_queue(&accounts, &recorder, &drive).await;
    assert!(!matches!(
        replay_snapshot(&accounts, "int-1", 1, 90_000),
        Ok(Expired)
    ));

    db.execute("UPDATE recordings SET deleted_by = 'operator'", [])
        .unwrap();
    assert!(matches!(
        replay_snapshot(&accounts, "int-1", 1, 90_000),
        Ok(Expired)
    ));
}

#[tokio::test]
async fn retention_cleanup_script_lists_exhausted_and_failed_cleanup_rows() {
    assert!(
        std::fs::read_to_string("./scripts/recording-cleanup.sh")
            .unwrap()
            .contains(&format!("\nretention={RETENTION_SECONDS}\n")),
        "recording-cleanup.sh retention must match RETENTION_SECONDS"
    );

    if std::process::Command::new("sh")
        .args(["-c", "command -v sqlite3"])
        .output()
        .map(|out| !out.status.success())
        .unwrap_or(true)
    {
        assert_ne!(
            std::env::var("CI").as_deref(),
            Ok("true"),
            "sqlite3 is not installed, and CI must run this test"
        );
        return;
    }

    let (scratch, accounts, _, _, recorder) = harness("retention-script-states");
    let recording = transferring(&accounts, &recorder).await;
    let db = scratch.path().to_str().unwrap();
    let conn = scratch.open();
    let list_at = |now: i64| {
        let out = std::process::Command::new("sh")
            .arg("./scripts/recording-cleanup.sh")
            .args(["--db", db, "--now", &now.to_string()])
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    conn.execute(
        "UPDATE recordings
         SET state = 'cleanup_failed', error = 'cleanup_failed', expires_at = 1
         WHERE id = ?1",
        [&recording.id],
    )
    .unwrap();
    assert_eq!(
        list_at(9),
        recording.id,
        "cleanup_failed is due whatever its expires_at"
    );

    conn.execute(
        "UPDATE recordings
         SET state = 'failed', error = 'drive_failed', updated_at = 1000, expires_at = 1
         WHERE id = ?1",
        [&recording.id],
    )
    .unwrap();
    let window = 1000 + RETENTION_SECONDS;
    assert_eq!(
        list_at(window - 1),
        "nothing is due for deletion",
        "drive_failed is not due before the retention window"
    );
    assert_eq!(
        list_at(window),
        recording.id,
        "drive_failed is due a retention window after its last failure"
    );

    let expired = std::process::Command::new("sh")
        .arg("./scripts/recording-cleanup.sh")
        .args(["--db", db, "--now", "1050", "--expire", &recording.id])
        .output()
        .unwrap();
    assert!(expired.status.success(), "{expired:?}");
    assert_eq!(
        list_at(1050),
        recording.id,
        "--expire makes a failed row due at once"
    );
}

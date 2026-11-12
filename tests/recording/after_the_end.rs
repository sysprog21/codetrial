//! A recording that has already ended, and what the provider is and is not
//! asked about it.

use codetrial::recording::{
    Clock, DELIVERY_BACKOFF_SECONDS, Failure, RETENTION_SECONDS, RecordingState, delete_recording,
    deliver_recording, due_for_deletion, enqueue_delivery, record_egress_id, recording_by_id,
    run_delivery_queue, sweep_recordings,
};

use std::sync::Arc;

use codetrial::recording::{BoxFuture, Recorder, RecordingProvider, StartEgress};

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
            "UPDATE recordings SET state = 'cleanup_failed', error = 'cleanup_failed'
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

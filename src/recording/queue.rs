use std::ops::ControlFlow;
use std::sync::Arc;

use rusqlite::OptionalExtension;

use crate::accounts::{Accounts, blocking};

use super::store::{SELECT_RECORDING, row_to_recording};
use super::*;

/// Three attempts, at zero, one minute and five. The first is the delivery
/// itself; a Drive call that failed twice in six minutes is failing for a
/// reason another minute will not fix, and the bytes are still in the staging
/// bucket for an operator to retry deliberately.
pub const DELIVERY_ATTEMPTS: i64 = 3;
pub const DELIVERY_BACKOFF_SECONDS: [i64; 2] = [60, 300];

/// How long a claim is honoured before another worker may take the row.
///
/// A worker that died with the process holds its claim forever otherwise, and
/// the recording sits in `transferring` until the sweeper abandons it.
///
/// An hour, which is longer than any upload this pipeline produces: a
/// forty-five minute interview at the configured bitrate is a few hundred
/// megabytes, and even a slow link finishes inside it. The number matters
/// because a claim that expires under a running upload is exactly how two
/// workers end up uploading at once, and the resumable session the first one
/// holds is invisible to the second.
pub const DELIVERY_CLAIM_SECONDS: i64 = 3600;

/// Put a recording in line for delivery.
///
/// Called when a recording reaches `transferring`, and idempotent: a webhook
/// LiveKit sent twice must not become two deliveries. `run_after` is now,
/// because the first attempt is the delivery itself rather than a retry.
pub fn enqueue_delivery(accounts: &Accounts, recording_id: &str, now: i64) -> rusqlite::Result<()> {
    accounts.with(|connection| {
        connection.execute(
            "
        INSERT INTO delivery_queue (recording_id, attempts, run_after, created_at, updated_at)
        VALUES (?1, 0, ?2, ?2, ?2)
        ON CONFLICT (recording_id) DO NOTHING
        ",
            (recording_id, now),
        )?;
        Ok(())
    })
}

/// Take one due row, or nothing.
///
/// The claim and the attempt count move in the same statement as the selection,
/// so two workers cannot both take the same row: SQLite serializes the writes,
/// and the second one finds a row whose `claimed_at` no longer matches what it
/// selected on.
///
/// Rows are due when `run_after` has passed and they are either unclaimed or
/// claimed longer ago than a delivery can take. The recording itself is read
/// back with the claim, because a queue row on its own says nothing about what
/// to deliver.
pub fn claim_delivery(
    accounts: &Accounts,
    now: i64,
) -> rusqlite::Result<Option<(Recording, String)>> {
    accounts.with(|connection| {
        let transaction = rusqlite::Transaction::new_unchecked(
            connection,
            rusqlite::TransactionBehavior::Immediate,
        )?;

        // The claim is a fresh token every time, so a worker whose claim
        // expired cannot finish or reschedule the row its replacement now
        // holds: its writes name a claim the row no longer has. A failure here
        // is the system's random source, which is not a condition to deliver
        // through: without a token this claim could be written over by the
        // worker it replaced.
        let claim = crate::accounts::random_token(16)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let claimed: Option<String> = transaction
            .query_row(
                "
        UPDATE delivery_queue
        SET claimed_at = ?1, claim = ?3, attempts = attempts + 1, updated_at = ?1
        WHERE recording_id = (
            SELECT recording_id FROM delivery_queue
            WHERE run_after <= ?1
              AND (claimed_at IS NULL OR claimed_at <= ?2)
            ORDER BY run_after
            LIMIT 1
        )
        RETURNING recording_id
        ",
                (now, now - DELIVERY_CLAIM_SECONDS, &claim),
                |row| row.get(0),
            )
            .optional()?;
        let Some(recording_id) = claimed else {
            transaction.commit()?;
            return Ok(None);
        };
        let recording = transaction
            .query_row(
                &format!("{SELECT_RECORDING} WHERE id = ?1"),
                [&recording_id],
                row_to_recording,
            )
            .optional()?;
        transaction.commit()?;
        Ok(recording.map(|recording| (recording, claim)))
    })
}

/// How many attempts a queued delivery has already had.
pub fn delivery_attempts(accounts: &Accounts, recording_id: &str) -> rusqlite::Result<Option<i64>> {
    accounts.with(|connection| {
        connection
            .query_row(
                "SELECT attempts FROM delivery_queue WHERE recording_id = ?1",
                [recording_id],
                |row| row.get(0),
            )
            .optional()
    })
}

/// The row is done with, whichever way it ended.
///
/// Named with the claim it was done under. A worker whose claim expired while
/// it worked would otherwise delete the row its replacement is holding, and the
/// replacement's delivery would finish into a queue that had forgotten it.
pub fn finish_delivery_work(
    accounts: &Accounts,
    recording_id: &str,
    claim: &str,
) -> rusqlite::Result<()> {
    accounts.with(|connection| {
        connection.execute(
            "DELETE FROM delivery_queue WHERE recording_id = ?1 AND claim = ?2",
            (recording_id, claim),
        )?;
        Ok(())
    })
}

/// Release a failed claim so the next attempt can happen later.
///
/// Returns whether there is another attempt. When there is not, the row is gone
/// and the caller is the one that has to fail the recording: a queue row for a
/// delivery nobody will attempt again is work that never happens, and the
/// sweeper would eventually abandon the recording with a less useful reason.
pub fn reschedule_delivery(
    accounts: &Accounts,
    recording_id: &str,
    claim: &str,
    error: &str,
    now: i64,
) -> rusqlite::Result<Reschedule> {
    accounts.with(|connection| {
        // One transaction, because the read and the write are one decision: a
        // row reclaimed between them would be read as this worker's and written
        // as somebody else's.
        let transaction = rusqlite::Transaction::new_unchecked(
            connection,
            rusqlite::TransactionBehavior::Immediate,
        )?;

        // Under this claim, or not at all. A row that has been reclaimed is
        // somebody else's work now, and rescheduling it would push their
        // attempt into the future for a failure that was not theirs.
        let attempts: Option<i64> = transaction
            .query_row(
                "SELECT attempts FROM delivery_queue WHERE recording_id = ?1 AND claim = ?2",
                (recording_id, claim),
                |row| row.get(0),
            )
            .optional()?;

        // Not this worker's row any more: it was reclaimed while this attempt
        // ran, or finished by somebody else. Saying "exhausted" here would fail
        // a recording another worker is in the middle of delivering, and take
        // its Drive file with it.
        let Some(attempts) = attempts else {
            return Ok(Reschedule::NotOurs);
        };
        let Some(backoff) = DELIVERY_BACKOFF_SECONDS.get(attempts.max(1) as usize - 1) else {
            // The row stays, still claimed. Deleting it here would leave a
            // `transferring` recording with an empty queue until the caller
            // fails it, which is exactly what the sweeper's repair looks for:
            // it would queue the row again and the next claim would reopen it,
            // turning an operator-only retry into an automatic one. The caller
            // deletes it after the failure is written down.
            transaction.commit()?;
            return Ok(Reschedule::Exhausted);
        };
        transaction.execute(
            "
        UPDATE delivery_queue
        SET claimed_at = NULL, claim = NULL, run_after = ?2, error = ?3, updated_at = ?4
        WHERE recording_id = ?1 AND claim = ?5
        ",
            (recording_id, now + backoff, error, now, claim),
        )?;
        transaction.commit()?;
        Ok(Reschedule::Again)
    })
}

/// What became of a failed attempt. Three answers, because the caller does
/// something different with each and a bool could only carry two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reschedule {
    /// Due again after the backoff.
    Again,
    /// Out of attempts. The caller fails the recording.
    Exhausted,
    /// The row is somebody else's now. The caller does nothing at all.
    NotOurs,
}

/// How long a delivered recording is readable before its permission expires and
/// the retention sweep deletes it. The contract's twenty-four hours, in one
/// place, because the Drive permission and the deletion deadline have to be the
/// same number or one of them is a lie.
pub const RETENTION_SECONDS: i64 = 24 * 60 * 60;

/// What one pass of the delivery queue did, for the caller to log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DeliveryOutcome {
    pub delivered: usize,
    pub retrying: usize,
    pub failed: usize,
}

/// Work the queue until there is nothing due.
///
/// This is what makes completion asynchronous: the webhook that ends a
/// recording queues the delivery and answers, and the upload happens here,
/// under a claim, with a schedule behind it.
pub async fn run_delivery_queue(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    delivery: &dyn DeliveryProvider,
) -> DeliveryOutcome {
    let mut outcome = DeliveryOutcome::default();
    loop {
        let now = recorder.clock.now();
        let claimed = {
            let accounts = accounts.clone();
            match blocking(move || claim_delivery(&accounts, now)).await {
                Ok(claimed) => claimed,
                Err(error) => {
                    eprintln!("WARNING: the delivery queue could not read its work: {error}");
                    return outcome;
                }
            }
        };
        let Some((recording, claim)) = claimed else {
            return outcome;
        };
        if deliver_claimed(
            accounts,
            recorder,
            delivery,
            recording,
            &claim,
            &mut outcome,
        )
        .await
        .is_break()
        {
            return outcome;
        }
    }
}

/// One claimed row, from reopening it to releasing it.
///
/// `Break` stops the whole pass rather than this row: it is the database
/// failing to answer, and the next row would ask it the same question. A row
/// this gives up on leaves its claim to expire, which is what makes a later
/// pass pick it up again.
async fn deliver_claimed(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    delivery: &dyn DeliveryProvider,
    recording: Recording,
    claim: &str,
    outcome: &mut DeliveryOutcome,
) -> ControlFlow<()> {
    let recording = reopen_if_asked(accounts, recorder, recording).await;

    // A row that left `transferring` while it waited: a withdrawal, a kill
    // switch, an operator. Its media is somebody else's problem now, and
    // delivering it would hand out a file that has been recalled.
    if recording.state != RecordingState::Transferring {
        release_claim(accounts, &recording.id, claim).await;
        return ControlFlow::Continue(());
    }

    // Read from the row rather than from the account, and read now rather than
    // at start: the address was copied into the row when the recording began,
    // which is the address the candidate consented with.
    let recipient = {
        let accounts = accounts.clone();
        let id = recording.id.clone();
        blocking(move || recipient_of(&accounts, &id)).await
    };
    let recipient = match recipient {
        Ok(Some(recipient)) => recipient,

        // A read that failed is a bad moment for the database, not a recording
        // without an address. The claim is left to expire, which is what makes
        // the next pass pick this row up again.
        Err(error) => {
            eprintln!("WARNING: a delivery could not read its recipient: {error}");
            return ControlFlow::Break(());
        }

        // No address on a `transferring` row is a state the schema's own
        // `CHECK` does not have, so this is a row somebody wrote around it.
        // There is nobody to share with and no retry that invents one, so it is
        // failed with the code an operator can act on.
        Ok(None) => {
            outcome.failed += 1;
            fail_and_release(accounts, recorder, &recording.id, claim).await;
            audit(
                "recording_failed",
                &recording.id,
                &[
                    ("reason", Failure::Drive.as_str()),
                    ("recovery", Failure::Drive.recovery()),
                    ("step", "recipient"),
                ],
            );
            return ControlFlow::Continue(());
        }
    };

    let delivered = deliver_recording(
        accounts,
        recorder,
        delivery,
        &recording,
        &recipient,
        RETENTION_SECONDS,
    )
    .await;
    let Err(failure) = delivered else {
        outcome.delivered += 1;
        release_claim(accounts, &recording.id, claim).await;
        return ControlFlow::Continue(());
    };

    let again = {
        let accounts = accounts.clone();
        let id = recording.id.clone();
        let reason = failure.as_str().to_string();
        let claim = claim.to_string();

        // Read again rather than reused. The claim was taken before the upload,
        // and an attempt that took longer than the backoff would schedule its
        // retry in the past.
        let failed_at = recorder.clock.now();
        blocking(move || reschedule_delivery(&accounts, &id, &claim, &reason, failed_at)).await
    };
    match again {
        Ok(Reschedule::Again) => {
            outcome.retrying += 1;
            return ControlFlow::Continue(());
        }

        // Reclaimed while this attempt ran, or already finished. Failing the
        // recording here would stop a delivery another worker is in the middle
        // of and delete the file it uploaded.
        Ok(Reschedule::NotOurs) => return ControlFlow::Continue(()),
        Ok(Reschedule::Exhausted) => {}
        Err(error) => {
            eprintln!("WARNING: a delivery could not be rescheduled: {error}");
            return ControlFlow::Continue(());
        }
    }

    // Out of attempts, or a queue row that has gone. The recording is failed
    // here rather than left for the sweeper, because `drive_failed` says what
    // an operator can do about it and `abandoned` does not.
    outcome.failed += 1;
    if fail_and_release(accounts, recorder, &recording.id, claim).await {
        audit(
            "recording_failed",
            &recording.id,
            &[
                ("reason", Failure::Drive.as_str()),
                ("recovery", Failure::Drive.recovery()),
                ("attempts", &DELIVERY_ATTEMPTS.to_string()),
            ],
        );
    }
    ControlFlow::Continue(())
}

/// A delivery an operator asked for again.
///
/// `retry_delivery` is the recovery `drive_failed` names, and this is where it
/// happens: the row goes back to `transferring` and the attempt runs like any
/// other. The guard is in the statement rather than in the transition table,
/// because "failed for this one reason" is not a state the table has and giving
/// it one would make every other failure re-openable too.
async fn reopen_if_asked(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    recording: Recording,
) -> Recording {
    if recording.state != RecordingState::Failed {
        return recording;
    }
    let reopened = {
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let id = recording.id.clone();
        blocking(move || reopen_delivery(&accounts, clock.as_ref(), &id)).await
    };
    match reopened {
        Ok(Some(reopened)) => reopened,
        _ => recording,
    }
}

/// Drops the queue row without touching the recording.
async fn release_claim(accounts: &Arc<Accounts>, recording_id: &str, claim: &str) {
    let accounts = accounts.clone();
    let id = recording_id.to_string();
    let claim = claim.to_string();
    let _ = blocking(move || finish_delivery_work(&accounts, &id, &claim)).await;
}

/// Fails the recording, then drops its queue row, and says whether this call is
/// the one that moved it.
///
/// The order is the point. Releasing first leaves a `transferring` recording
/// with an empty queue, which is exactly what the sweeper's repair looks for,
/// and the attempt nobody authorized happens by itself.
///
/// A database that cannot write the transition leaves the row queued and the
/// next pass tries again. That is the right answer for a moment of trouble and
/// no answer at all for a database that stays broken, which is a condition
/// nothing else here survives either.
async fn fail_and_release(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    recording_id: &str,
    claim: &str,
) -> bool {
    let accounts = accounts.clone();
    let clock = recorder.clock.clone();
    let id = recording_id.to_string();
    let claim = claim.to_string();
    let moved = blocking(move || {
        let moved = transition(
            &accounts,
            clock.as_ref(),
            &id,
            RecordingState::Failed,
            Some(Failure::Drive.as_str()),
        );
        if moved.is_ok() {
            let _ = finish_delivery_work(&accounts, &id, &claim);
        }
        moved
    })
    .await;
    matches!(moved, Ok(Some((_, true))))
}

/// Queue any `transferring` recording the queue does not know about.
///
/// Idempotent and cheap: the insert names rows the queue is missing, so a
/// database where nothing went wrong writes nothing.
pub fn requeue_orphaned_deliveries(accounts: &Accounts, now: i64) -> rusqlite::Result<usize> {
    accounts.with(|connection| {
        connection.execute(
            "
        INSERT INTO delivery_queue (recording_id, attempts, run_after, created_at, updated_at)
        SELECT id, 0, ?1, ?1, ?1 FROM recordings
        WHERE state = 'transferring'
          AND id NOT IN (SELECT recording_id FROM delivery_queue)
        ",
            [now],
        )
    })
}

/// Put a recording that failed its delivery back in line for one.
///
/// Only from `failed` with `drive_failed`, and only while the media is still
/// there: a tombstoned row has nothing to deliver, and a recording that failed
/// for any other reason has no file in the staging bucket to try again with.
///
/// Returns the reopened row, or `None` when the guard refused, so the caller
/// works from what the database says rather than from what it asked for.
pub fn reopen_delivery(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
) -> rusqlite::Result<Option<Recording>> {
    accounts.with(|connection| {
        let moved = connection.execute(
            "
        UPDATE recordings
        SET state = 'transferring', updated_at = ?2
        WHERE id = ?1 AND state = 'failed' AND error = ?3 AND deleted_at IS NULL
        ",
            (recording_id, clock.now(), Failure::Drive.as_str()),
        )?;
        if moved == 0 {
            return Ok(None);
        }
        audit(
            "recording_delivery_reopened",
            recording_id,
            &[("reason", Failure::Drive.as_str())],
        );
        connection
            .query_row(
                &format!("{SELECT_RECORDING} WHERE id = ?1"),
                [recording_id],
                row_to_recording,
            )
            .map(Some)
    })
}

fn recipient_of(accounts: &Accounts, recording_id: &str) -> rusqlite::Result<Option<String>> {
    accounts.with(|connection| {
        connection.query_row(
            "SELECT recipient_email FROM recordings WHERE id = ?1",
            [recording_id],
            |row| row.get(0),
        )
    })
}

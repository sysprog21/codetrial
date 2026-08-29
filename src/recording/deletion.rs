//! Deleting a recording's media, and the tombstone row that outlives it.

use std::sync::Arc;

use crate::accounts::{Accounts, blocking};

use super::store::{Handle, clear_handle, delivery_handles};
use super::*;

/// Revokes, deletes, and tombstones, in that order.
///
/// A failure anywhere is `cleanup_failed`, which is terminal for the pipeline
/// and an alert for a person: the media outlived the attempt to delete it, and
/// nothing automatic is going to fix that.
/// One step of releasing a recording's media: what to ask the provider for, and
/// the handle the row stops naming once that lands.
///
/// The call is named rather than built. `DeliveryProvider` returns boxed
/// futures that happen to be lazy today, so building all three up front would
/// work, but nothing in the trait says an implementation cannot do its work
/// before it boxes anything. Deleting the file before the revoke landed is not
/// a failure worth resting on that.
struct Release<'a> {
    // `'static` and `Copy`: both cross into the blocking pool, which takes only
    // what it can own, and neither is anything a caller passed.
    step: &'static str,
    clear_step: &'static str,
    handle: Handle,
    call: Call<'a>,
}

/// Which provider call a [`Release`] makes when its turn comes.
enum Call<'a> {
    Revoke {
        drive_file_id: &'a str,
        permission_id: &'a str,
    },
    DeleteFile(&'a str),
    DeleteObject(&'a str),
}

pub async fn delete_recording(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    delivery: &dyn DeliveryProvider,
    recording: &Recording,
    deleted_by: &str,
) -> Result<RecordingState, String> {
    let stored = {
        let accounts = accounts.clone();
        let id = recording.id.clone();
        blocking(move || delivery_handles(&accounts, &id)).await
    };
    let Ok((drive_file_id, drive_permission_id, gcs_object)) = stored else {
        return Err("could not read the delivery handles".to_string());
    };

    // Three steps, each written down as it succeeds. One call covering all
    // three records nothing between them, so a revoke that worked and a file
    // deletion that did not could only be retried from the start: the row still
    // named a permission that was already gone, and the retry's revoke failed
    // on it forever. Clearing each handle as its step lands makes the row the
    // progress, and every step idempotent by construction, because a handle
    // that is null is a step nothing repeats.
    let mut releases = Vec::new();
    if let (Some(file_id), Some(permission_id)) =
        (drive_file_id.as_deref(), drive_permission_id.as_deref())
    {
        releases.push(Release {
            step: "revoke",
            clear_step: "clear_permission",
            handle: Handle::DrivePermission,
            call: Call::Revoke {
                drive_file_id: file_id,
                permission_id,
            },
        });
    }
    if let Some(file_id) = drive_file_id.as_deref() {
        releases.push(Release {
            step: "delete_file",
            clear_step: "clear_file",
            handle: Handle::DriveFile,
            call: Call::DeleteFile(file_id),
        });
    }
    if let Some(object) = gcs_object.as_deref() {
        releases.push(Release {
            step: "delete_object",
            clear_step: "clear_object",
            handle: Handle::GcsObject,
            call: Call::DeleteObject(object),
        });
    }

    let mut failed = None;
    for Release {
        step,
        clear_step,
        handle,
        call,
    } in releases
    {
        let made = match call {
            Call::Revoke {
                drive_file_id,
                permission_id,
            } => delivery.revoke(drive_file_id, permission_id).await,
            Call::DeleteFile(drive_file_id) => delivery.delete_file(drive_file_id).await,
            Call::DeleteObject(gcs_object) => delivery.delete_object(gcs_object).await,
        };
        if let Err(error) = made {
            failed = Some((step, error));
            break;
        }
        let accounts = accounts.clone();
        let id = recording.id.clone();
        if let Err(error) = blocking(move || clear_handle(&accounts, &id, handle)).await {
            // The handle is gone from the provider and the row still names it.
            // Reported rather than swallowed: the next pass repeats a call that
            // will `404`, which is harmless, and an operator reading
            // `cleanup_failed` learns the database is the thing that is broken.
            failed = Some((clear_step, error.to_string()));
            break;
        }
    }

    if let Some((step, error)) = failed {
        let error = format!("{step}: {error}");

        if fail_cleanup(accounts, recorder, &recording.id, Failure::Cleanup).await {
            audit(
                "recording_cleanup_failed",
                &recording.id,
                &[
                    ("step", step),
                    ("error", &error),
                    ("reason", Failure::Cleanup.as_str()),
                    ("action", Failure::Cleanup.recovery()),
                ],
            );
        }
        return Err(error);
    }

    let tombstoned = {
        let accounts = accounts.clone();
        let clock = recorder.clock.clone();
        let id = recording.id.clone();
        let deleted_by = deleted_by.to_string();
        blocking(move || tombstone(&accounts, clock.as_ref(), &id, &deleted_by)).await
    };

    // A zero-row update is somebody else's deletion, not this one's. Reporting
    // `recording_deleted` for it would put two deletions in the log for one
    // file.
    if let Ok(false) = tombstoned {
        return Ok(RecordingState::Deleted);
    }
    if let Err(error) = tombstoned {
        // The media is gone and the row still says otherwise, holding the
        // recipient address and handles for files that no longer exist. That is
        // a person's problem, and the row has to say so rather than sitting in
        // `ready` describing a recording nobody can watch.
        if fail_cleanup(accounts, recorder, &recording.id, Failure::Tombstone).await {
            audit(
                "recording_cleanup_failed",
                &recording.id,
                &[
                    ("step", "tombstone"),
                    ("error", &error.to_string()),
                    ("reason", Failure::Tombstone.as_str()),
                    ("action", Failure::Tombstone.recovery()),
                    ("media", "already_deleted"),
                ],
            );
        }
        return Err(format!("the deletion could not be written down: {error}"));
    }
    audit("recording_deleted", &recording.id, &[("by", deleted_by)]);
    Ok(RecordingState::Deleted)
}

/// Moves a recording to `cleanup_failed`, and says whether this call is the one
/// that moved it.
///
/// The transition first, and the audit line only for whoever gets `true`. A
/// concurrent deletion can win, and a line telling an operator that cleanup
/// failed for a row that is `deleted` sends them looking for a file that is not
/// there.
async fn fail_cleanup(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    recording_id: &str,
    failure: Failure,
) -> bool {
    let accounts = accounts.clone();
    let clock = recorder.clock.clone();
    let id = recording_id.to_string();
    let moved = blocking(move || {
        transition(
            &accounts,
            clock.as_ref(),
            &id,
            RecordingState::CleanupFailed,
            Some(failure.as_str()),
        )
    })
    .await;
    matches!(moved, Ok(Some((_, true))))
}

/// Records the deletion and gives up everything that described what was
/// deleted.
///
/// One statement, because the schema's own `CHECK` refuses a half-tombstone: a
/// row with `deleted_at` set and a recipient address still in it is not a state
/// this table has.
fn tombstone(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
    deleted_by: &str,
) -> rusqlite::Result<bool> {
    let now = clock.now();
    accounts.with(|connection| {
        let transaction = rusqlite::Transaction::new_unchecked(
            connection,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let changed = transaction.execute(
            "
        UPDATE recordings
        SET state = 'deleted',
            deleted_at = ?2,
            deleted_by = ?3,
            room_name = NULL,
            recipient_email = NULL,
            gcs_object = NULL,
            drive_file_id = NULL,
            drive_permission_id = NULL,
            updated_at = ?2
        WHERE id = ?1 AND deleted_at IS NULL
        ",
            (recording_id, now, deleted_by),
        )?;

        // The replay goes with the media, in the same write. The consent
        // disclosure promises deletion, and a replay is the interview without
        // the video: what was said, what was typed, what the tests said.
        // Keeping it after the file is gone keeps the interview.
        if changed > 0 {
            transaction.execute(
                "
        DELETE FROM replay_events
        WHERE interview_id = (SELECT interview_id FROM recordings WHERE id = ?1)
        ",
                [recording_id],
            )?;
        }
        transaction.commit()?;
        Ok(changed > 0)
    })
}

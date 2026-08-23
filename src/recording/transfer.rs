//! Moving one finished recording out of the staging bucket and into the
//! recipient's hands, and undoing the parts of that which landed when a
//! later part did not.

use std::sync::Arc;

use crate::accounts::{Accounts, blocking};

use super::store::delivery_handles;
use super::*;

pub async fn deliver_recording(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    delivery: &dyn DeliveryProvider,
    recording: &Recording,
    recipient_email: &str,
    retention_seconds: i64,
) -> Result<RecordingState, Failure> {
    let transfer = Transfer {
        accounts,
        recorder,
        delivery,
        recording,
        gcs_object: gcs_object_path(&recorder.config.gcs_prefix, &recording.id),
    };

    transfer.stage_object().await?;
    let uploaded = transfer.upload_file().await?;

    // Read from the clock here rather than taken from the caller. The deadline
    // is twenty-four hours of readable file, and an upload that took forty
    // minutes would otherwise hand the candidate twenty-three hours and twenty
    // minutes of it.
    let expires_at = recorder.clock.now() + retention_seconds;
    let shared = transfer
        .share_file(&uploaded, recipient_email, expires_at)
        .await?;

    transfer.finish(&uploaded, &shared, expires_at).await?;
    audit("recording_ready", &recording.id, &[]);
    Ok(RecordingState::Ready)
}

/// One delivery in progress.
///
/// The four steps all need the same five things, and threading them through
/// four signatures made each one a list of arguments that mostly restated which
/// delivery this was. What a step's own signature says now is what that step
/// needs on top of the delivery it belongs to.
struct Transfer<'a> {
    accounts: &'a Arc<Accounts>,
    recorder: &'a Recorder,
    delivery: &'a dyn DeliveryProvider,
    recording: &'a Recording,
    gcs_object: String,
}

/// The file this delivery is working with, and whether this call is the one
/// that created it. An earlier attempt that uploaded and then failed to share
/// leaves a file that must be reused rather than made again, and `made_here`
/// is what the cleanup path needs to know: what this call did not create, it
/// does not get to delete.
struct Uploaded {
    drive_file_id: String,
    made_here: bool,

    /// A permission an earlier attempt already granted. Carried out of the
    /// upload step because it is read from the same row, and asking Drive to
    /// grant the same person the same access twice is either a duplicate grant
    /// or an error, and neither is what a retry wanted.
    existing_permission: Option<String>,
}

/// The same, for the permission.
struct Shared {
    permission_id: String,
    made_here: bool,
}

impl Transfer<'_> {
    /// The audit line every failed step writes, and the failure it returns. One
    /// method because the four steps have to agree on the reason and the
    /// recovery they hand an operator; only the step and the error differ.
    fn failed(&self, step: &str, error: &str) -> Failure {
        audit(
            "recording_delivery_failed",
            &self.recording.id,
            &[
                ("step", step),
                ("error", error),
                ("reason", Failure::Drive.as_str()),
                ("recovery", Failure::Drive.recovery()),
            ],
        );
        Failure::Drive
    }

    /// Records the failure against the row, so the retry count and the reason
    /// belong to this delivery.
    async fn record_failure(&self) {
        record_delivery_failure(self.accounts, self.recorder, &self.recording.id).await;
    }

    /// Undoes the parts of this delivery that landed, for a row somebody else
    /// has already moved on from.
    async fn clean_up_after_losing(
        &self,
        step: &str,
        handles: (Option<&str>, Option<(&str, &str)>),
    ) {
        clean_up_after_losing(
            self.accounts,
            self.delivery,
            &self.recording.id,
            step,
            handles,
            &self.gcs_object,
        )
        .await;
    }

    /// Writes the staged object down before anything remote happens.
    ///
    /// It is derivable from the recording id, but the deletion path reads it
    /// from the row, and a delivery that ended anywhere except success used to
    /// leave that column empty: the bytes stayed in the bucket with nothing
    /// naming them.
    async fn stage_object(&self) -> Result<(), Failure> {
        let recorded = {
            let accounts = self.accounts.clone();
            let id = self.recording.id.clone();
            let gcs_object = self.gcs_object.clone();
            blocking(move || set_gcs_object(&accounts, &id, &gcs_object)).await
        };
        let error = match recorded {
            Ok(true) => return Ok(()),

            // `Err` is not `Ok(false)`. A database failure is not a confirmed
            // state race, and treating it as one sends the cleanup path after
            // media that is still wanted.
            Err(_) => "the staged object could not be written down",

            // Nothing remote happens until this is written down, and it is
            // written only for a recording that is still transferring. Creating
            // media whose cleanup handle was never recorded, or creating it at
            // all for a recording that already ended, are the two failures this
            // ordering exists to prevent.
            Ok(false) => "the staged object could not be written to a transferring recording",
        };
        let failure = self.failed("record_object", error);
        self.record_failure().await;
        Err(failure)
    }

    /// Puts the media in the Shared Drive, or finds what an earlier attempt put
    /// there, and writes the file id down before the share can name it.
    async fn upload_file(&self) -> Result<Uploaded, Failure> {
        // An earlier attempt may have uploaded and then failed to share.
        // Uploading again would leave that file in the Shared Drive with
        // nothing naming it.
        let handles = {
            let accounts = self.accounts.clone();
            let id = self.recording.id.clone();
            blocking(move || delivery_handles(&accounts, &id)).await
        };
        let Ok((existing, existing_permission, _)) = handles else {
            // Not knowing whether a file exists is not permission to make
            // another one. A read failure read as "no file" is how the
            // no-orphan guarantee stops holding during exactly the trouble it
            // is for.
            let failure = self.failed("read_handles", "the delivery handles could not be read");
            self.record_failure().await;
            return Err(failure);
        };
        let made_here = existing.is_none();
        let drive_file_id = match existing {
            Some(drive_file_id) => drive_file_id,
            None => {
                let filename = format!("{}.mp4", self.recording.id);
                match self
                    .delivery
                    .transfer(&self.gcs_object, &filename, &self.recording.id)
                    .await
                {
                    Ok(id) => id,
                    Err(error) => {
                        let failure = self.failed("transfer", &error);
                        self.record_failure().await;
                        return Err(failure);
                    }
                }
            }
        };

        // Written before the share, and its failure stops the delivery. A file
        // in the Shared Drive that no row names is a file nothing can find and
        // nothing can delete, and going on to share it would hand somebody a
        // link to it.
        //
        // Only when this call uploaded. Reusing an id the row already holds and
        // then writing it again would fail the `IS NULL` guard and send this
        // delivery off to delete the file it was reusing.
        let stored = if made_here {
            let accounts = self.accounts.clone();
            let id = self.recording.id.clone();
            let drive_file_id = drive_file_id.clone();
            blocking(move || set_drive_file(&accounts, &id, &drive_file_id)).await
        } else {
            Ok(true)
        };
        match stored {
            Ok(true) => Ok(Uploaded {
                drive_file_id,
                made_here,
                existing_permission,
            }),
            Err(_) => {
                // A write that failed is not a state race. The file stays where
                // it is, because the row may still want it and the next attempt
                // reuses the id once this can be written down.
                let failure =
                    self.failed("record_file", "the Drive file id could not be written down");
                self.record_failure().await;
                Err(failure)
            }

            // The row left `transferring` while the upload ran, which a
            // withdrawal or a kill switch does. The file this call created
            // belongs to nobody, so it goes rather than sitting in the Shared
            // Drive with nothing naming it.
            Ok(false) => {
                let failure = self.failed(
                    "record_file",
                    "the Drive file id could not be written to a transferring recording",
                );

                // No `record_failure` here. That write is guarded on
                // `transferring`, which is the state the row that beat this one
                // is in, so recording a failure would put this delivery's error
                // and retry count on somebody else's delivery.
                self.clean_up_after_losing(
                    "record_file",
                    (made_here.then_some(drive_file_id.as_str()), None),
                )
                .await;
                Err(failure)
            }
        }
    }

    /// Grants the recipient time-limited access, or keeps the grant an earlier
    /// attempt made, and writes the permission id down before returning.
    async fn share_file(
        &self,
        uploaded: &Uploaded,
        recipient_email: &str,
        expires_at: i64,
    ) -> Result<Shared, Failure> {
        let drive_file_id = uploaded.drive_file_id.as_str();
        let made_here = uploaded.existing_permission.is_none();
        let permission_id = match uploaded.existing_permission.clone() {
            Some(permission_id) => permission_id,
            None => match self
                .delivery
                .share(drive_file_id, recipient_email, expires_at)
                .await
            {
                Ok(id) => id,
                Err(error) => {
                    let failure = self.failed("share", &error);
                    self.record_failure().await;
                    return Err(failure);
                }
            },
        };

        // The permission id first, on its own, for the same reason the file id
        // was: a permission nothing names is one nothing can revoke.
        let stored = if made_here {
            let accounts = self.accounts.clone();
            let id = self.recording.id.clone();
            let permission_id = permission_id.clone();
            blocking(move || set_drive_permission(&accounts, &id, &permission_id)).await
        } else {
            Ok(true)
        };
        match stored {
            Ok(true) => Ok(Shared {
                permission_id,
                made_here,
            }),
            Err(_) => {
                let failure = self.failed(
                    "record_permission",
                    "the permission id could not be written down",
                );
                self.record_failure().await;
                Err(failure)
            }
            Ok(false) => {
                let failure = self.failed(
                    "record_permission",
                    "the permission id could not be written to a transferring recording",
                );
                self.clean_up_after_losing(
                    "record_permission",
                    (
                        uploaded.made_here.then_some(drive_file_id),
                        made_here.then_some((drive_file_id, permission_id.as_str())),
                    ),
                )
                .await;
                Err(failure)
            }
        }
    }

    /// Marks the recording delivered, which is the write that makes it `Ready`.
    async fn finish(
        &self,
        uploaded: &Uploaded,
        shared: &Shared,
        expires_at: i64,
    ) -> Result<(), Failure> {
        let finished = {
            let accounts = self.accounts.clone();
            let clock = self.recorder.clock.clone();
            let id = self.recording.id.clone();
            blocking(move || finish_delivery(&accounts, clock.as_ref(), &id, expires_at)).await
        };
        match finished {
            Ok(true) => Ok(()),
            Err(_) => {
                let failure = self.failed("finish", "the delivery could not be written down");
                self.record_failure().await;
                Err(failure)
            }

            // A lost race, not a failure of this delivery: somebody else moved
            // the row and their state is the right one, so recording a failure
            // over it would overwrite a newer answer with an older complaint.
            // What this delivery made goes with it, because nothing is going to
            // come back for a file belonging to a recording that ended.
            Ok(false) => {
                audit(
                    "recording_delivery_lost",
                    &self.recording.id,
                    &[("step", "finish")],
                );
                let drive_file_id = uploaded.drive_file_id.as_str();
                self.clean_up_after_losing(
                    "finish",
                    (
                        uploaded.made_here.then_some(drive_file_id),
                        shared
                            .made_here
                            .then_some((drive_file_id, shared.permission_id.as_str())),
                    ),
                )
                .await;
                Err(Failure::Drive)
            }
        }
    }
}

/// What a delivery that lost its row may delete.
///
/// Two questions, not one, and they have different answers. A Drive file this
/// invocation uploaded and could not record is nobody's and always goes. The
/// staged object belongs to the recording, and goes only when the recording is
/// over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CleanupScope {
    /// What this call created. `false` only when the row could not be read,
    /// because not knowing is not permission to delete.
    mine: bool,
    /// The staged object, which another delivery may still be reading from and
    /// which retention owns.
    object: bool,
}

async fn cleanup_scope(accounts: &Arc<Accounts>, recording_id: &str) -> CleanupScope {
    let accounts = accounts.clone();
    let recording_id = recording_id.to_string();
    let Ok(Some(row)) = blocking(move || recording_by_id(&accounts, &recording_id)).await else {
        return CleanupScope {
            mine: false,
            object: false,
        };
    };
    if row.error.as_deref().and_then(Failure::parse) == Some(Failure::Drive) {
        // A delivery the queue will come back to. Its staged object stays; the
        // file this invocation uploaded and never recorded does not belong to
        // that retry and would be orphaned by keeping it.
        return CleanupScope {
            mine: true,
            object: false,
        };
    }
    match row.state {
        RecordingState::Transferring | RecordingState::Ready => CleanupScope {
            mine: true,
            object: false,
        },
        RecordingState::Failed | RecordingState::CleanupFailed | RecordingState::Deleted => {
            CleanupScope {
                mine: true,
                object: true,
            }
        }
        _ => CleanupScope {
            mine: false,
            object: false,
        },
    }
}

/// The cleanup one lost delivery is allowed to perform.
///
/// `mine` is what this invocation created: a file it uploaded, and a permission
/// it granted paired with the file that permission is on. A file it reused
/// belongs to an earlier attempt and the staged object belongs to the
/// recording, so deleting either because this call lost a race takes media from
/// somebody who still wants it.
async fn clean_up_after_losing(
    accounts: &Arc<Accounts>,
    delivery: &dyn DeliveryProvider,
    recording_id: &str,
    step: &str,
    mine: (Option<&str>, Option<(&str, &str)>),
    gcs_object: &str,
) {
    let scope = cleanup_scope(accounts, recording_id).await;
    let (drive_file_id, permission) = if scope.mine { mine } else { (None, None) };
    let object = scope.object.then_some(gcs_object);
    if drive_file_id.is_none() && permission.is_none() && object.is_none() {
        return;
    }
    let outcome = delivery
        .revoke_and_delete(drive_file_id, permission, object)
        .await;
    let deleted = outcome.is_ok();
    report_cleanup(recording_id, step, outcome);

    // A handle that names a file this call just deleted is worse than no
    // handle: a later attempt reads it, skips the upload, and tries to share
    // something that is not there. Guarded on the value, so a row that has
    // moved on to some other file keeps it.
    if deleted && let Some(drive_file_id) = drive_file_id {
        let accounts = accounts.clone();
        let recording_id = recording_id.to_string();
        let drive_file_id = drive_file_id.to_string();
        let _ = blocking(move || forget_drive_file(&accounts, &recording_id, &drive_file_id)).await;
    }
}

/// Give up a Drive handle whose file is gone.
fn forget_drive_file(
    accounts: &Accounts,
    recording_id: &str,
    drive_file_id: &str,
) -> rusqlite::Result<()> {
    accounts.with(|connection| {
        connection.execute(
            "
        UPDATE recordings
        SET drive_file_id = NULL, drive_permission_id = NULL
        WHERE id = ?1 AND drive_file_id = ?2 AND deleted_at IS NULL
        ",
            (recording_id, drive_file_id),
        )?;
        Ok(())
    })
}

/// A cleanup that itself failed, said out loud.
///
/// These run on paths already handling a failure, and swallowing them meant a
/// withdrawal that won mid-delivery could leave a Drive file and staged bytes
/// behind with nothing recorded and nobody told.
fn report_cleanup(recording_id: &str, step: &str, outcome: Result<(), String>) {
    if let Err(error) = outcome {
        audit(
            "recording_cleanup_failed",
            recording_id,
            &[
                ("step", step),
                ("error", &error),
                ("reason", Failure::Cleanup.as_str()),
                ("action", Failure::Cleanup.recovery()),
            ],
        );
    }
}

/// A delivery attempt that did not work, without moving the recording.
///
/// The row stays in `transferring` because the bytes are still in the staging
/// bucket and another attempt is the recovery. Moving it to `failed` advertised
/// `retry_delivery` from a state no transition could leave.
async fn record_delivery_failure(
    accounts: &Arc<Accounts>,
    recorder: &Recorder,
    recording_id: &str,
) {
    let accounts = accounts.clone();
    let clock = recorder.clock.clone();
    let recording_id = recording_id.to_string();
    let _ = blocking(move || {
        accounts.with(|connection| {
            connection.execute(
                "
        UPDATE recordings
        SET error = ?2, retries = retries + 1, updated_at = ?3
        WHERE id = ?1 AND state = 'transferring'
        ",
                (&recording_id, Failure::Drive.as_str(), clock.now()),
            )?;
            Ok(())
        })
    })
    .await;
}

/// `false` when the row was no longer `transferring`, or already names a file.
///
/// The `IS NULL` guard is what stops two concurrent deliveries from both
/// uploading and the second overwriting the first, which orphans a file in the
/// Shared Drive. The loser deletes what it made.
fn set_drive_file(
    accounts: &Accounts,
    recording_id: &str,
    drive_file_id: &str,
) -> rusqlite::Result<bool> {
    accounts.with(|connection| {
        let changed = connection.execute(
            "
        UPDATE recordings SET drive_file_id = ?2
        WHERE id = ?1 AND state = 'transferring' AND drive_file_id IS NULL
        ",
            (recording_id, drive_file_id),
        )?;
        Ok(changed > 0)
    })
}

fn set_drive_permission(
    accounts: &Accounts,
    recording_id: &str,
    permission_id: &str,
) -> rusqlite::Result<bool> {
    accounts.with(|connection| {
        let changed = connection.execute(
            "
        UPDATE recordings SET drive_permission_id = ?2
        WHERE id = ?1 AND state = 'transferring' AND drive_permission_id IS NULL
        ",
            (recording_id, permission_id),
        )?;
        Ok(changed > 0)
    })
}

/// `false` when the row is not `transferring`, which is the whole check: a
/// delivery called against a recording that already ended must not reach the
/// provider at all.
fn set_gcs_object(
    accounts: &Accounts,
    recording_id: &str,
    gcs_object: &str,
) -> rusqlite::Result<bool> {
    accounts.with(|connection| {
        let changed = connection.execute(
            "UPDATE recordings SET gcs_object = ?2 WHERE id = ?1 AND state = 'transferring'",
            (recording_id, gcs_object),
        )?;
        Ok(changed > 0)
    })
}

/// `false` when the row was no longer `transferring`, which is somebody else
/// having moved it.
fn finish_delivery(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
    expires_at: i64,
) -> rusqlite::Result<bool> {
    let now = clock.now();
    accounts.with(|connection| {
        let changed = connection.execute(
            "
        UPDATE recordings
        SET expires_at = ?2,
            state = 'ready',
            -- Cleared, or a delivery that failed once and then worked would be
            -- `ready` still carrying `drive_failed`, and the status route would
            -- tell a candidate their recording had not been delivered.
            error = NULL,
            ready_at = ?3,
            updated_at = ?3
        WHERE id = ?1 AND state = 'transferring'
        ",
            (recording_id, expires_at, now),
        )?;
        Ok(changed > 0)
    })
}

use rusqlite::OptionalExtension;

use crate::accounts::Accounts;

use super::*;

/// A recording, as the routes and the sweeper read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    pub id: String,
    pub account_id: i64,
    pub interview_id: String,
    pub room_name: Option<String>,
    pub egress_id: Option<String>,
    pub state: RecordingState,
    pub error: Option<String>,
    pub retries: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

pub(super) const SELECT_RECORDING: &str = "
        SELECT id, account_id, interview_id, room_name, egress_id, state, error, retries,
               created_at, updated_at
        FROM recordings
";

pub(super) fn row_to_recording(row: &rusqlite::Row<'_>) -> rusqlite::Result<Recording> {
    let state: String = row.get(5)?;
    Ok(Recording {
        id: row.get(0)?,
        account_id: row.get(1)?,
        interview_id: row.get(2)?,
        room_name: row.get(3)?,
        egress_id: row.get(4)?,

        // An unreadable state is not a state. It comes back as `Failed` rather
        // than panicking, because a row written by a newer binary must not take
        // the sweeper down with it.
        state: RecordingState::parse(&state).unwrap_or(RecordingState::Failed),
        error: row.get(6)?,
        retries: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

/// Why a start was refused. Each one is a different sentence to the candidate
/// and a different thing for an operator to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartRefusal {
    /// No such interview, not this account's, or consent has been withdrawn.
    NoConsent,
    /// The interview never claimed a room, so there is nothing to record.
    NoRoom,
    KillSwitch,
    /// This account already has a recording the pipeline owes work on.
    AlreadyActive,
    Database,
}

/// The row that says a recording exists, written before the provider is called.
///
/// The `bool` is whether this call inserted the row, and it is the whole answer
/// to "may I call the provider". Two concurrent starts both see a `starting`
/// row with no egress id, and if both treated that as work to do there would be
/// two Egress jobs. Only the caller that inserted it starts; a row whose
/// creator never got that far is the sweeper's.
///
/// Every rule the insert has to obey is inside the insert. Consent, ownership
/// and a claimed room are its `WHERE`; one-per-interview and
/// one-active-per-account are unique indexes. Reading them first and inserting
/// second is serialized by this process's one connection and by nothing across
/// two, and the reads that remain exist only to say *why* an insert that landed
/// nowhere landed nowhere.
pub fn begin_recording(
    accounts: &Accounts,
    clock: &dyn Clock,
    interview_id: &str,
    account_id: i64,
    recording_id: &str,
    recipient_email: &str,
    kill_switch: bool,
) -> Result<(Recording, bool), StartRefusal> {
    let now = clock.now();
    accounts
        .with(|connection| {
            // The switch is read before anything else, including before an
            // existing recording is handed back. An operator who throws it
            // means every start, not only the ones that have not happened yet.
            if kill_switch {
                return Ok(Err(StartRefusal::KillSwitch));
            }

            // Consent, ownership and a claimed room are the insert's own
            // `WHERE`, so nothing can change between checking them and writing.
            let inserted = connection.execute(
                "
        INSERT INTO recordings (
            id, account_id, interview_id, room_name, idempotency_key,
            recipient_email, state, created_at, updated_at
        )
        SELECT ?1, ?2, ?3, interviews.room_name, ?3, ?4, 'starting', ?5, ?5
        FROM interviews
        WHERE interviews.id = ?3
          AND interviews.account_id = ?2
          AND interviews.consent_withdrawn_at IS NULL
          AND interviews.room_name IS NOT NULL
        ",
                (
                    recording_id,
                    account_id,
                    interview_id,
                    recipient_email,
                    now,
                ),
            );
            if inserted.as_ref().is_ok_and(|rows| *rows > 0) {
                let recording = connection.query_row(
                    &format!("{SELECT_RECORDING} WHERE id = ?1"),
                    [recording_id],
                    row_to_recording,
                )?;
                return Ok(Ok((recording, true)));
            }

            // From here on, everything is about explaining a refusal. An
            // existing recording for this interview is the common one, and it
            // is not a refusal at all: a repeated start finds the first
            // caller's row rather than making a second Egress job.
            let existing = connection
                .query_row(
                    &format!("{SELECT_RECORDING} WHERE interview_id = ?1"),
                    [interview_id],
                    row_to_recording,
                )
                .optional()?;
            if let Some(existing) = existing {
                return Ok(if existing.account_id == account_id {
                    Ok((existing, false))
                } else {
                    Err(StartRefusal::NoConsent)
                });
            }

            let interview: Option<(Option<String>, Option<i64>)> = connection
                .query_row(
                    "SELECT room_name, consent_withdrawn_at FROM interviews WHERE id = ?1 AND account_id = ?2",
                    (interview_id, account_id),
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((room_name, withdrawn)) = interview else {
                return Ok(Err(StartRefusal::NoConsent));
            };
            if withdrawn.is_some() {
                return Ok(Err(StartRefusal::NoConsent));
            }
            if room_name.is_none() {
                return Ok(Err(StartRefusal::NoRoom));
            }

            let active: i64 = connection.query_row(
                "
        SELECT COUNT(*) FROM recordings
        WHERE account_id = ?1
          AND state IN ('starting', 'recording', 'finalizing', 'transferring')
        ",
                [account_id],
                |row| row.get(0),
            )?;
            if active > 0 {
                return Ok(Err(StartRefusal::AlreadyActive));
            }

            // The insert obeyed every rule this function knows and still did
            // not land, so whatever stopped it is a database problem rather
            // than a refusal.
            Err(inserted.err().unwrap_or(rusqlite::Error::QueryReturnedNoRows))
        })
        .unwrap_or_else(|error| {
            eprintln!("could not begin a recording: {error}");
            Err(StartRefusal::Database)
        })
}

/// Records the provider's answer.
///
/// The id is written whatever the state, and the state moves only from
/// `starting`. Guarding the id on the state as well was a leak: a stop that
/// landed first left the row in `finalizing`, the id was discarded, and the
/// Egress job it named could no longer be stopped by anything.
///
/// It is never overwritten. A slow first attempt and a sweeper retry can both
/// come back with a job, and the second to arrive must not replace the first:
/// that would leave a job nothing can name. `false` means somebody else's id is
/// already there, and the caller has a job of its own to stop.
pub fn record_egress_id(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
    egress_id: &str,
) -> rusqlite::Result<bool> {
    let now = clock.now();
    accounts.with(|connection| {
        let changed = connection.execute(
            "
        UPDATE recordings
        SET egress_id = ?2,
            state = CASE WHEN state = 'starting' THEN 'recording' ELSE state END,
            updated_at = ?3
        WHERE id = ?1 AND egress_id IS NULL
        ",
            (recording_id, egress_id, now),
        )?;
        if changed > 0 {
            return Ok(true);
        }

        // Already ours is not a loss. A retry that adopted the same job, or an
        // answer that arrived twice, should not send the caller off to stop it.
        let current: Option<String> = connection.query_row(
            "SELECT egress_id FROM recordings WHERE id = ?1",
            [recording_id],
            |row| row.get(0),
        )?;
        Ok(current.as_deref() == Some(egress_id))
    })
}

/// Moves a recording, refusing any move the table does not have.
///
/// Returns the state the row is in afterwards and whether this call is what put
/// it there. The state is not always the one that was asked for: a duplicate
/// webhook asks for a transition that has already happened, and the answer to
/// that is the current state rather than an error.
///
/// The `bool` is what makes a concurrent stop safe. Two callers can both find a
/// recording active, and only the one that moved it should talk to the
/// provider; the other has nothing left to do.
pub fn transition(
    accounts: &Accounts,
    clock: &dyn Clock,
    recording_id: &str,
    next: RecordingState,
    error: Option<&str>,
) -> rusqlite::Result<Option<(RecordingState, bool)>> {
    let now = clock.now();
    accounts.with(|connection| {
        let current: Option<String> = connection
            .query_row(
                "SELECT state FROM recordings WHERE id = ?1",
                [recording_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(current) = current.as_deref().and_then(RecordingState::parse) else {
            return Ok(None);
        };

        // Already there. The transition is allowed, and nothing moved, which is
        // the difference a duplicate stop rides on: the second caller has
        // nothing to tell the provider.
        if current == next {
            return Ok(Some((current, false)));
        }
        if !current.may_become(next) {
            return Ok(Some((current, false)));
        }

        // Guarded on the state it was read in, so two callers racing on one
        // recording cannot both come away believing they moved it.
        let changed = connection.execute(
            "
        UPDATE recordings
        SET state = ?2, error = COALESCE(?3, error), updated_at = ?4
        WHERE id = ?1 AND state = ?5
        ",
            (recording_id, next.as_str(), error, now, current.as_str()),
        )?;
        if changed > 0 {
            return Ok(Some((next, true)));
        }

        // Somebody else moved it first, so the state to report is theirs and
        // not the one that was asked for. Reporting `next` here told `/end`
        // that a recording was `finalizing` when it had already failed.
        let actual: String = connection.query_row(
            "SELECT state FROM recordings WHERE id = ?1",
            [recording_id],
            |row| row.get(0),
        )?;
        Ok(RecordingState::parse(&actual).map(|state| (state, false)))
    })
}

pub fn recording_by_id(
    accounts: &Accounts,
    recording_id: &str,
) -> rusqlite::Result<Option<Recording>> {
    accounts.with(|connection| {
        connection
            .query_row(
                &format!("{SELECT_RECORDING} WHERE id = ?1"),
                [recording_id],
                row_to_recording,
            )
            .optional()
    })
}

pub fn recording_for_interview(
    accounts: &Accounts,
    interview_id: &str,
    account_id: i64,
) -> rusqlite::Result<Option<Recording>> {
    accounts.with(|connection| {
        connection
            .query_row(
                &format!("{SELECT_RECORDING} WHERE interview_id = ?1 AND account_id = ?2"),
                (interview_id, account_id),
                row_to_recording,
            )
            .optional()
    })
}

pub fn recording_for_room(
    accounts: &Accounts,
    room_name: &str,
) -> rusqlite::Result<Option<Recording>> {
    accounts.with(|connection| {
        connection
            .query_row(
                &format!("{SELECT_RECORDING} WHERE room_name = ?1"),
                [room_name],
                row_to_recording,
            )
            .optional()
    })
}

pub fn recording_for_egress(
    accounts: &Accounts,
    egress_id: &str,
) -> rusqlite::Result<Option<Recording>> {
    accounts.with(|connection| {
        connection
            .query_row(
                &format!("{SELECT_RECORDING} WHERE egress_id = ?1"),
                [egress_id],
                row_to_recording,
            )
            .optional()
    })
}

/// Rows the pipeline still owes work on that nothing has touched for a while.
///
/// "Touched", not "created": a recording that has been running for forty
/// minutes is a recording, and one whose row has not moved for fifteen is a
/// process that died holding it.
pub fn stale_active(
    accounts: &Accounts,
    older_than: Option<i64>,
) -> rusqlite::Result<Vec<Recording>> {
    accounts.with(|connection| {
        // `None` means every active row, which is what the kill switch wants.
        // Written as a null-checked comparison rather than as a sentinel
        // timestamp, because `updated_at <= i64::MAX` reads as a bug even when
        // it is not one.
        //
        // A recording the delivery queue still owes work on is not stale,
        // whatever its `updated_at` says. An upload runs for up to half an hour
        // and touches nothing while it does, and a second delivery waits behind
        // it, so without this the sweeper fails a transfer that is running and
        // the worker then deletes the file it had just uploaded. Every
        // outstanding row, not only the claimed one, for the queued case. The
        // kill switch still takes them, because a kill switch that waited for
        // an upload is not one.
        let mut statement = connection.prepare(&format!(
            "{SELECT_RECORDING}
        WHERE state IN ('starting', 'recording', 'finalizing', 'transferring')
          AND (?1 IS NULL OR updated_at <= ?1)
          AND (?1 IS NULL OR id NOT IN (SELECT recording_id FROM delivery_queue))
        "
        ))?;
        let rows = statement
            .query_map([older_than], row_to_recording)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Whether this webhook still needs dealing with.
///
/// `false` means it has already been applied. LiveKit retries anything it did
/// not get a 2xx for, and a retry is a fresh, validly signed message with a new
/// `createdAt` and the same `id`, so the id is the only thing that can tell
/// them apart.
///
/// An event that was claimed and never applied comes back `true`. Deleting the
/// claim on failure was the first answer and it had a hole: if the delete
/// itself failed, or the process died between the two, the id stayed and the
/// redelivery did nothing. A null `applied_at` is an attempt that did not
/// finish, and there is no second write to lose.
pub fn claim_webhook_event(
    accounts: &Accounts,
    event_id: &str,
    received_at: i64,
) -> rusqlite::Result<bool> {
    accounts.with(|connection| {
        connection.execute(
            "
        INSERT INTO recording_events (event_id, received_at) VALUES (?1, ?2)
        ON CONFLICT(event_id) DO NOTHING
        ",
            (event_id, received_at),
        )?;
        let applied: Option<i64> = connection.query_row(
            "SELECT applied_at FROM recording_events WHERE event_id = ?1",
            [event_id],
            |row| row.get(0),
        )?;
        Ok(applied.is_none())
    })
}

/// Marks an event dealt with, so its retries are no-ops.
pub fn mark_webhook_applied(
    accounts: &Accounts,
    event_id: &str,
    applied_at: i64,
) -> rusqlite::Result<()> {
    accounts.with(|connection| {
        connection.execute(
            "UPDATE recording_events SET applied_at = ?2 WHERE event_id = ?1",
            (event_id, applied_at),
        )?;
        Ok(())
    })
}

/// How long the ledger remembers. LiveKit gives up retrying long before this,
/// and a table that only grows is a table that eventually matters.
pub const EVENT_RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;

pub fn prune_webhook_events(accounts: &Accounts, before: i64) -> rusqlite::Result<usize> {
    accounts.with(|connection| {
        connection.execute(
            "DELETE FROM recording_events WHERE received_at < ?1",
            [before],
        )
    })
}

pub(super) fn delivery_handles(
    accounts: &Accounts,
    recording_id: &str,
) -> rusqlite::Result<(Option<String>, Option<String>, Option<String>)> {
    accounts.with(|connection| {
        connection.query_row(
            "SELECT drive_file_id, drive_permission_id, gcs_object FROM recordings WHERE id = ?1",
            [recording_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
    })
}

/// Give up one handle, now that what it named is gone.
///
/// The column name arrives as a literal from `deletion`, which holds all three
/// call sites, rather than as a value from anywhere else. The `unreachable!`
/// is what keeps that true: a caller cannot name a column this function was not
/// written for without failing loudly.
pub(super) fn clear_handle(
    accounts: &Accounts,
    recording_id: &str,
    column: &str,
) -> rusqlite::Result<()> {
    let statement = match column {
        "drive_permission_id" => "UPDATE recordings SET drive_permission_id = NULL WHERE id = ?1",
        "drive_file_id" => "UPDATE recordings SET drive_file_id = NULL WHERE id = ?1",
        "gcs_object" => "UPDATE recordings SET gcs_object = NULL WHERE id = ?1",
        other => unreachable!("no handle called {other}"),
    };
    accounts.with(|connection| {
        connection.execute(statement, [recording_id])?;
        Ok(())
    })
}

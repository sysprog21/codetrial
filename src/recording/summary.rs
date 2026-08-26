use rusqlite::OptionalExtension;

use crate::accounts::Accounts;

use super::store::{SELECT_RECORDING, row_to_recording};
use super::*;

/// One page of an account's recordings, newest first.
///
/// Paged by `created_at` and `id` rather than by offset: an offset shifts under
/// a row being inserted or deleted, which for this list means an interview
/// appearing twice or not at all while a candidate scrolls. The cursor is the
/// last row of the previous page.
pub const RECORDING_PAGE: i64 = 20;

pub fn recordings_for_account(
    accounts: &Accounts,
    account_id: i64,
    before: Option<(i64, String)>,
) -> rusqlite::Result<Vec<Recording>> {
    accounts.with(|connection| {
        // The first page starts above every row rather than at a sentinel that
        // has to be reasoned about: `created_at < i64::MAX` is true for any
        // timestamp this table can hold.
        let (created_at, id) = before.unwrap_or((i64::MAX, String::new()));
        let mut statement = connection.prepare(&format!(
            "{SELECT_RECORDING}
        WHERE account_id = ?1
          AND (created_at < ?2 OR (created_at = ?2 AND id < ?3))
        ORDER BY created_at DESC, id DESC
        LIMIT ?4
        "
        ))?;
        let rows = statement
            .query_map(
                (account_id, created_at, id, RECORDING_PAGE + 1),
                row_to_recording,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// What a reader may know about one recording.
///
/// Deliberately not a `Recording`: that carries the room name and the egress
/// id, and a history page has no use for either. What is here is what a person
/// asks about their own interview, plus the two dates that say whether it can
/// still be watched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingSummary {
    pub id: String,
    pub interview_id: String,
    pub state: RecordingState,
    pub error: Option<String>,
    pub created_at: i64,
    pub ready_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub deleted_at: Option<i64>,
    pub quota_exceeded: bool,
}

pub fn recording_summary(
    accounts: &Accounts,
    recording_id: &str,
    account_id: i64,
) -> rusqlite::Result<Option<RecordingSummary>> {
    accounts.with(|connection| {
        connection
            .query_row(
                "
        SELECT id, interview_id, state, error, created_at, ready_at, expires_at,
               deleted_at, quota_exceeded
        FROM recordings WHERE id = ?1 AND account_id = ?2
        ",
                (recording_id, account_id),
                |row| {
                    let state: String = row.get(2)?;
                    Ok(RecordingSummary {
                        id: row.get(0)?,
                        interview_id: row.get(1)?,
                        state: RecordingState::parse(&state).unwrap_or(RecordingState::Failed),
                        error: row.get(3)?,
                        created_at: row.get(4)?,
                        ready_at: row.get(5)?,
                        expires_at: row.get(6)?,
                        deleted_at: row.get(7)?,
                        quota_exceeded: row.get::<_, i64>(8)? != 0,
                    })
                },
            )
            .optional()
    })
}

/// Recordings whose media should not exist any more.
///
/// Two reasons, and they are different promises. A recording past `expires_at`
/// has had its twenty-four hours. A recording whose candidate withdrew consent
/// has had a person say stop, and it has no deadline at all: nothing set one,
/// because it may never have been delivered.
///
/// Withdrawal is read from the interview as well as from the recording's own
/// error, because the two say it at different moments. A withdrawal during the
/// interview fails the recording with `consent_withdrawn`; one after delivery
/// leaves a `ready` row the transition table will not move, and the interview
/// is the only place that says stop.
///
/// `cleanup_failed` rows come back too. That is what makes a partial deletion
/// resumable: the handles the last attempt could not remove are still on the
/// row, and the ones it did remove are not.
///
/// A recording whose Egress job was never confirmed stopped is not due, however
/// old it is. Tombstoning gives up the room name, which is the only thing the
/// orphan sweep has to stop that job with, and a job nobody can stop keeps
/// writing media into a bucket this pass has just emptied. The sweep below
/// stops it first; the next pass takes the row.
pub fn due_for_deletion(accounts: &Accounts, now: i64) -> rusqlite::Result<Vec<Recording>> {
    accounts.with(|connection| {
        let mut statement = connection.prepare(&format!(
            "{SELECT_RECORDING}
        WHERE deleted_at IS NULL
          AND state IN ('ready', 'failed', 'cleanup_failed')
          AND (egress_id IS NULL OR stopped_at IS NOT NULL)
          AND ((expires_at IS NOT NULL AND expires_at <= ?1)
               OR error = ?2
               OR interview_id IN (
                   SELECT id FROM interviews WHERE consent_withdrawn_at IS NOT NULL
               ))
        ORDER BY id
        "
        ))?;
        let rows = statement
            .query_map((now, Failure::ConsentWithdrawn.as_str()), row_to_recording)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Why this recording's media is being deleted, in the words the tombstone
/// keeps. `operator` is the third, and it is a person running the cleanup
/// script rather than anything this module decides.
pub(super) fn deletion_reason(accounts: &Accounts, recording: &Recording) -> &'static str {
    if recording.error.as_deref() == Some(Failure::ConsentWithdrawn.as_str()) {
        return "consent_withdrawn";
    }
    let asked: rusqlite::Result<(Option<String>, bool)> = accounts.with(|connection| {
        connection.query_row(
            "
        SELECT deleted_by,
               (SELECT consent_withdrawn_at IS NOT NULL FROM interviews WHERE id = interview_id)
        FROM recordings WHERE id = ?1
        ",
            [&recording.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
    });
    let (asked_by, withdrawn) = asked.unwrap_or((None, false));
    if withdrawn {
        return "consent_withdrawn";
    }

    // A person who brought this deletion forward said so on the row before the
    // sweeper found it. Their word for it survives, because "expiry" would say
    // the deadline came and it did not.
    if asked_by.as_deref() == Some("operator") {
        return "operator";
    }
    "expiry"
}

//! The reports an account holds, and the quota that bounds them.
//!
//! Split out of [`super`] because the quota is the whole story: a full account
//! refuses a new report and still rewrites its own, which is one rule spread
//! across two queries and an enum, and it read as unrelated to the session and
//! interview code it used to sit between.

use super::*;

/// A candidate accumulates one report per interview, so a few hundred is a
/// career. Without a ceiling `/api/reports` is an authenticated write with no
/// rate limit and no cap, and a client that picks its own row ids can grow the
/// database until the disk runs out.
pub const MAX_REPORTS_PER_USER: i64 = 200;

/// Three outcomes the caller has to tell apart: a bool could not distinguish
/// "that id is someone else's" from "you are out of room", and they need
/// different status codes.
#[derive(Debug, PartialEq, Eq)]
pub enum ReportSave {
    Saved,
    NotOwner,
    AtCapacity,
}

/// Newest first, and totally ordered.
///
/// Anything reading this list as a chronology, which the lobby does, needs the
/// order to be total. Both timestamps are whole seconds, so two reports written
/// inside one second tie on either key and SQLite is free to return them in any
/// order, putting that read at the mercy of the query plan. The id breaks the
/// tie so the answer is at least the same one twice.
pub fn list_reports(accounts: &Accounts, user_id: i64) -> rusqlite::Result<Vec<Value>> {
    accounts.with(|connection| {
        let mut statement = connection.prepare(
            "
        SELECT id, problem_id, payload, created_at, updated_at
        FROM reports
        WHERE user_id = ?1
        ORDER BY updated_at DESC, created_at DESC, id DESC
        ",
        )?;
        statement
            .query_map([user_id], |row| {
                let payload: String = row.get(2)?;
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "problemId": row.get::<_, String>(1)?,
                    "payload": serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null),
                    "createdAt": row.get::<_, i64>(3)?,
                    "updatedAt": row.get::<_, i64>(4)?,
                }))
            })?
            .collect()
    })
}

/// Deletes every saved report owned by one account and returns the row count.
/// Interviews, replay events, and recordings have separate lifecycles and are
/// deliberately untouched.
pub fn delete_reports(accounts: &Accounts, user_id: i64) -> rusqlite::Result<usize> {
    accounts
        .with(|connection| connection.execute("DELETE FROM reports WHERE user_id = ?1", [user_id]))
}

pub fn save_report(
    accounts: &Accounts,
    user_id: i64,
    id: &str,
    problem_id: &str,
    payload: &Value,
) -> rusqlite::Result<ReportSave> {
    accounts.with(|connection| {
        let now = current_epoch_seconds() as i64;

        // Who holds the id, and how full the account is, in one round trip.
        // Counting only the rows matching both columns could not tell "that id
        // is someone else's" from "that id is new", so a full account was told
        // it was out of room when the real answer was that the id was taken.
        let (owner, total): (Option<i64>, i64) = connection.query_row(
            "
        SELECT
            (SELECT user_id FROM reports WHERE id = ?1),
            (SELECT COUNT(*) FROM reports WHERE user_id = ?2)
        ",
            (id, user_id),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        match owner {
            Some(existing) if existing != user_id => return Ok(ReportSave::NotOwner),

            // Only a new row counts against the quota. Rewriting a report the
            // candidate already owns has to keep working at the ceiling, or
            // finishing an interview would fail once the account filled up.
            Some(_) => {}
            None if total >= MAX_REPORTS_PER_USER => return Ok(ReportSave::AtCapacity),
            None => {}
        }
        let changed = connection.execute(
            "
        INSERT INTO reports (id, user_id, problem_id, payload, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?5)
        ON CONFLICT(id) DO UPDATE SET
            problem_id = excluded.problem_id,
            payload = excluded.payload,
            updated_at = excluded.updated_at
        WHERE reports.user_id = excluded.user_id
        ",
            (id, user_id, problem_id, payload.to_string().as_str(), now),
        )?;
        Ok(if changed > 0 {
            ReportSave::Saved
        } else {
            ReportSave::NotOwner
        })
    })
}

//! One candidate's booked interview: creating it, claiming its room, and
//! withdrawing the consent that lets it be recorded.
//!
//! Split out of [`super`] because these five queries share a quota and a room
//! claim that nothing else here touches, and reading any one of them means
//! reading the other four.

use super::*;

/// A career of interviews, bounded. `POST /api/interviews` is an authenticated
/// write with no other ceiling, and a client that calls it in a loop grows the
/// database until the disk does not.
pub const MAX_INTERVIEWS_PER_USER: i64 = 500;

/// One interview, and the consent that allows it to be recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interview {
    pub id: String,
    pub account_id: i64,
    pub consent_version: String,
    pub consent_at: i64,
    pub consent_withdrawn_at: Option<i64>,
    pub room_name: Option<String>,
}

/// Records consent and returns the interview it belongs to.
///
/// The caller supplies the id so that the value it hands the browser and the
/// value in the row cannot differ.
/// Records consent, or hands back the pending interview that already has it.
///
/// Reuse rather than a second row, because a reload is not a second consent: it
/// is the same person, the same wording, and an interview that has not started.
/// Always inserting meant every refresh and every refused token request spent
/// one of the account's rows for nothing.
///
/// The guarantee stops at the claim, and deliberately: once a token is minted
/// the interview is bound to a room, so a candidate whose LiveKit connection
/// then fails does start a fresh interview on reload. That is the right answer,
/// because the room they were given is not one they can rejoin.
///
/// Only a pending interview qualifies: not withdrawn, not already bound to a
/// room, and agreed to the wording currently being shown. A version bump
/// therefore takes fresh consent, which is the point of versioning it.
pub fn create_interview(
    accounts: &Accounts,
    id: &str,
    account_id: i64,
    consent_version: &str,
    now: i64,
) -> rusqlite::Result<Option<Interview>> {
    accounts.with(|connection| {
        const PENDING: &str = "
        SELECT id, consent_at FROM interviews
        WHERE account_id = ?1
          AND consent_version = ?2
          AND consent_withdrawn_at IS NULL
          AND room_name IS NULL
        ";
        let pending = |connection: &rusqlite::Connection| {
            connection
                .query_row(PENDING, (account_id, consent_version), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .optional()
        };

        // Three, because each retry needs another process to both win the
        // insert and spend the row before this one can read it. Losing that
        // race three times running is not a state worth writing more code for.
        for _ in 0..3 {
            if let Some((id, consent_at)) = pending(connection)? {
                return Ok(Some(Interview {
                    id,
                    account_id,
                    consent_version: consent_version.to_string(),

                    // The original time. They consented then, not on the
                    // reload.
                    consent_at,
                    consent_withdrawn_at: None,
                    room_name: None,
                }));
            }

            // The cap bounds the table, not a race: two processes inserting at
            // once can overshoot it by one, which is a row, and the alternative
            // is a write lock on every consent.
            let total: i64 = connection.query_row(
                "SELECT COUNT(*) FROM interviews WHERE account_id = ?1",
                [account_id],
                |row| row.get(0),
            )?;
            if total >= MAX_INTERVIEWS_PER_USER {
                return Ok(None);
            }

            // `DO NOTHING` rather than an error: another process may have
            // inserted its own pending row since the read above, and the
            // candidate should get that interview rather than a constraint
            // failure. Whether this insert landed is what decides which.
            let inserted = connection.execute(
                "
        INSERT INTO interviews (id, account_id, consent_version, consent_at)
        VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(account_id, consent_version)
            WHERE room_name IS NULL AND consent_withdrawn_at IS NULL
            DO NOTHING
        ",
                (id, account_id, consent_version, now),
            )?;
            if inserted > 0 {
                return Ok(Some(Interview {
                    id: id.to_string(),
                    account_id,
                    consent_version: consent_version.to_string(),
                    consent_at: now,
                    consent_withdrawn_at: None,
                    room_name: None,
                }));
            }
        }

        // Reported as a failure rather than as `None`, which the caller answers
        // with "this account is full". Losing the race is not being full, and a
        // candidate told the wrong thing looks for the wrong fix.
        Err(rusqlite::Error::QueryReturnedNoRows)
    })
}

/// Binds an interview to the one room it authorizes, or refuses.
///
/// `false` means the interview is gone, is somebody else's, has had its consent
/// withdrawn, or has already started a room. One consent is one interview: the
/// `room_name IS NULL` guard is what makes that true rather than aspirational,
/// and it is a single statement so two token requests racing on one id cannot
/// both win.
///
/// The consent check is repeated here rather than trusted from the caller's
/// earlier read. It costs nothing inside a statement that has to run anyway,
/// and it closes the window between the two.
pub fn claim_interview_room(
    accounts: &Accounts,
    id: &str,
    account_id: i64,
    room_name: &str,
) -> rusqlite::Result<bool> {
    accounts.with(|connection| {
        let changed = connection.execute(
            "
        UPDATE interviews SET room_name = ?3
        WHERE id = ?1
          AND account_id = ?2
          AND room_name IS NULL
          AND consent_withdrawn_at IS NULL
        ",
            (id, account_id, room_name),
        )?;
        Ok(changed > 0)
    })
}

/// The interview, only if this account owns it.
///
/// Ownership is part of the lookup rather than a check the caller remembers to
/// make. A missing interview and somebody else's come back the same way on
/// purpose: the difference is not the caller's business, and answering it
/// differently is a way to enumerate other people's ids.
pub fn interview_for_account(
    accounts: &Accounts,
    id: &str,
    account_id: i64,
) -> rusqlite::Result<Option<Interview>> {
    accounts.with(|connection| {
        let mut statement = connection.prepare(
            "
        SELECT id, account_id, consent_version, consent_at, consent_withdrawn_at, room_name
        FROM interviews
        WHERE id = ?1 AND account_id = ?2
        ",
        )?;
        let mut rows = statement.query((id, account_id))?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(Interview {
            id: row.get(0)?,
            account_id: row.get(1)?,
            consent_version: row.get(2)?,
            consent_at: row.get(3)?,
            consent_withdrawn_at: row.get(4)?,
            room_name: row.get(5)?,
        }))
    })
}

/// Gives a claimed room back.
///
/// The claim is spent before the interviewer is dispatched, so that two
/// requests racing on one interview cannot both start a room. A dispatcher that
/// refuses then leaves a consent bound to a room nobody will join, and the
/// candidate is told to retry a request that would be refused. This is that
/// retry working.
///
/// Guarded on the room it is giving back, so a release arriving late cannot
/// unbind a room some other request has since claimed.
///
/// The ceiling: a process that dies between the claim and the release leaves
/// the interview bound to a room that never ran, and the candidate has to
/// reload. That is one page reload against a lock nobody can take back, which
/// is the trade a crash-safe reservation would be bought with.
pub fn release_interview_room(
    accounts: &Accounts,
    id: &str,
    account_id: i64,
    room_name: &str,
) -> rusqlite::Result<()> {
    accounts.with(|connection| {
        connection.execute(
            "
        UPDATE interviews SET room_name = NULL
        WHERE id = ?1 AND account_id = ?2 AND room_name = ?3
        ",
            (id, account_id, room_name),
        )?;
        Ok(())
    })
}

/// Marks consent withdrawn, once.
///
/// Idempotent by construction: `COALESCE` keeps the first timestamp, which is
/// the one that matters, and `RETURNING` answers whether the interview exists
/// and belongs to this account rather than whether a row changed. One statement
/// rather than an update and a count, so there is no interleaving to reason
/// about at all.
pub fn withdraw_consent(
    accounts: &Accounts,
    id: &str,
    account_id: i64,
    now: i64,
) -> rusqlite::Result<bool> {
    accounts.with(|connection| {
        connection
            .query_row(
                "
        UPDATE interviews SET consent_withdrawn_at = COALESCE(consent_withdrawn_at, ?3)
        WHERE id = ?1 AND account_id = ?2
        RETURNING consent_withdrawn_at
        ",
                (id, account_id, now),
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map(|row| row.is_some())
    })
}

/// A typed handle is a label, not proof of anything, so it must never be the
/// key an account is found by: deriving the id from the login would mean typing
/// someone else's handle lands on their row and hands over their reports. Each
/// recorded login therefore gets a fresh id, and a negative one, because GitHub
/// ids are positive and the two kinds of account must never collide.
pub fn recorded_account_id() -> std::io::Result<i64> {
    let mut bytes = [0u8; 8];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| std::io::Error::other("could not read system entropy"))?;

    // Lands in `i64::MIN ..= -1` and cannot overflow, which negating a random
    // magnitude can. `github_id` is UNIQUE, so the one-in-2^63 repeat fails the
    // insert rather than quietly joining two people to one account.
    Ok(i64::MIN + (i64::from_be_bytes(bytes) & i64::MAX))
}

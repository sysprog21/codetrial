//! Signing in, staying signed in, and being swept when neither is true.
//!
//! The session id in the cookie is never what the database holds:
//! [`session_key`]
//! hashes it on the way in, so a copy of the file is not a set of live
//! sessions. Split out of [`super`] so that rule sits beside the four queries
//! that depend on it rather than in the middle of the account schema.

use super::*;

/// Thirty days. Long enough that a candidate booking an interview next week is
/// still signed in when they arrive.
pub const SESSION_TTL_SECONDS: i64 = 60 * 60 * 24 * 30;

/// Signs someone in, minting the session token the cookie carries.
///
/// `pub(crate)` deliberately. It was raised in review that a public
/// `create_session` plus a public `GitHubProfile` lets any in-process caller
/// mint an account marked verified, since `github_id > 0` proves only a
/// number's sign and not that GitHub said anything. Visibility is not a defence
/// against code in the same process, which can write the row directly, but this
/// function has exactly two callers and neither is outside this crate, so the
/// narrower spelling costs nothing and stops the shape from spreading.
///
/// The real invariant lives at those two callers: the OAuth callback is the
/// only one that supplies a positive id, and `record_login_handler` always
/// takes its id from `recorded_account_id`, which is negative by construction.
pub(crate) fn create_session(
    accounts: &Accounts,
    profile: &GitHubProfile,
) -> rusqlite::Result<String> {
    accounts.with(|connection| {
        let now = current_epoch_seconds() as i64;

        // A positive id came from GitHub and names a returning person, so it
        // updates the row it already owns. A negative one is a freshly minted
        // account for a self-declared handle and must never adopt an existing
        // row: if the random id ever repeats, the insert has to fail rather
        // than hand the new arrival somebody else's reports.
        const INSERT_USER: &str = "
        INSERT INTO users (github_id, login, avatar_url, email, email_verified, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?5, ?6, ?4, ?4)
    ";
        const ADOPT_EXISTING: &str = "
        ON CONFLICT(github_id) DO UPDATE SET
            login = excluded.login,
            avatar_url = excluded.avatar_url,
            email = excluded.email,
            email_verified = excluded.email_verified,
            updated_at = excluded.updated_at
    ";
        let statement = if profile.github_id < 0 {
            INSERT_USER.to_string()
        } else {
            format!("{INSERT_USER}{ADOPT_EXISTING}")
        };

        // A verified address is only ever written for a positive id, which is
        // an account GitHub vouched for. A self-declared login carries a
        // negative id and gets a NULL address and a zero flag, so there is no
        // path by which typing a handle produces a delivery target. Trimmed and
        // non-empty here, not only at the caller. This function is the
        // persistence boundary and it is public: `email_verified` is derived
        // from whether this is `Some`, so a positive account whose address is
        // nothing but spaces would be recorded as verified with nothing to
        // deliver to.
        let verified_email = profile
            .verified_email
            .as_deref()
            .map(str::trim)
            .filter(|email| !email.is_empty())
            .filter(|_| profile.github_id > 0);
        connection.execute(
            &statement,
            (
                profile.github_id,
                profile.login.as_str(),
                profile.avatar_url.as_deref(),
                now,
                verified_email,
                i64::from(verified_email.is_some()),
            ),
        )?;
        let user_id = connection.query_row(
            "SELECT id FROM users WHERE github_id = ?1",
            [profile.github_id],
            |row| row.get::<_, i64>(0),
        )?;
        let session_id = random_token(32).map_err(|_| rusqlite::Error::InvalidQuery)?;
        connection.execute(
            "
        INSERT INTO sessions (id, user_id, expires_at, created_at)
        VALUES (?1, ?2, ?3, ?4)
        ",
            (
                session_key(&session_id),
                user_id,
                now + SESSION_TTL_SECONDS,
                now,
            ),
        )?;
        Ok(session_id)
    })
}

pub fn session_user(
    accounts: &Accounts,
    session_id: &str,
) -> rusqlite::Result<Option<SignedInUser>> {
    accounts.with(|connection| {
        let now = current_epoch_seconds() as i64;
        let mut statement = connection.prepare(
            "
        SELECT users.id, users.login, users.avatar_url, users.email, users.email_verified
        FROM sessions
        JOIN users ON users.id = sessions.user_id
        WHERE sessions.id = ?1 AND sessions.expires_at > ?2
        ",
        )?;
        let mut rows = statement.query((session_key(session_id), now))?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };

        // The flag gates the column. A row with an address and a zero flag is
        // one nobody checked, and reading the address anyway is how an
        // unverified value becomes a delivery target one refactor later.
        let verified: i64 = row.get(4)?;
        Ok(Some(SignedInUser {
            id: row.get(0)?,
            login: row.get(1)?,
            avatar_url: row.get(2)?,
            verified_email: row.get::<_, Option<String>>(3)?.filter(|_| verified != 0),
        }))
    })
}

pub fn delete_session(accounts: &Accounts, session_id: &str) -> rusqlite::Result<()> {
    accounts.with(|connection| {
        connection.execute(
            "DELETE FROM sessions WHERE id = ?1",
            [session_key(session_id)],
        )?;
        Ok(())
    })
}

/// Deletes expired sessions and the throwaway accounts left behind by them.
///
/// `POST /api/login` mints a fresh negative account id per submission, so every
/// unverified sign-in inserted one `users` row and one `sessions` row and
/// nothing ever removed either. Rate limiting bounded the rate, not the total,
/// so the database grew for the lifetime of the deployment.
///
/// Only negative ids are collected. A positive id is a real GitHub account and
/// must outlive its sessions, and any account still owning a report is kept
/// whatever its id, because the report is the thing worth keeping.
///
/// Recordings are excluded for a harder reason than reports. Deleting an
/// account cascades into `interviews`, and `recordings` references that with
/// `ON DELETE RESTRICT`, so such a row would not be skipped, it would fail this
/// statement. No self-declared account can own a recording today, because
/// recording needs a verified address and that needs a positive id, but a sweep
/// whose correctness rests on a rule enforced two modules away is one refactor
/// from failing on every boot.
///
/// Returns the rows removed, so the caller can say so rather than sweeping
/// silently.
pub fn sweep_expired_sessions(accounts: &Accounts, now: i64) -> rusqlite::Result<(usize, usize)> {
    accounts.with(|connection| {
        // One transaction, because the two statements are one decision. The
        // mutex around this connection serializes this process and says nothing
        // about a second `codetrial web` on the same database: a recording
        // inserted between them makes the account delete fail under RESTRICT,
        // after the sessions have already gone.
        let transaction = connection.unchecked_transaction()?;
        let sessions = transaction.execute("DELETE FROM sessions WHERE expires_at <= ?1", [now])?;
        let users = transaction.execute(
            "
            DELETE FROM users
            WHERE github_id < 0
              AND id NOT IN (SELECT user_id FROM sessions)
              AND id NOT IN (SELECT user_id FROM reports)
              AND id NOT IN (SELECT account_id FROM recordings)
            ",
            [],
        )?;
        transaction.commit()?;
        Ok((sessions, users))
    })
}

/// What the `sessions` table stores. The cookie carries the token itself and
/// only its digest is written down, so a copy of the database is a list of
/// useless strings rather than thirty days of live logins.
///
/// Unsalted on purpose: the input is 32 bytes from the OS, so there is no
/// dictionary to precompute, and a per-row salt would cost the lookup the
/// primary-key index it currently rides on.
///
/// Public so integration fixtures that pin explicit user ids can still write
/// the row `create_session` would have written, rather than keeping a second
/// copy of this rule that drifts.
pub fn session_key(token: &str) -> String {
    use sha2::Digest;
    URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(token.as_bytes()))
}

pub fn random_token(bytes: usize) -> std::io::Result<String> {
    let mut token = vec![0; bytes];
    ring::rand::SystemRandom::new()
        .fill(&mut token)
        .map_err(|_| std::io::Error::other("could not read system entropy"))?;
    Ok(URL_SAFE_NO_PAD.encode(token))
}

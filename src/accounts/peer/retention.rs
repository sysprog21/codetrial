//! Closed peer practice is kept for a bounded time, then removed with
//! everything it owns.
use super::Accounts;

/// How long a closed session, its code snapshots and feedback, and a finished
/// lobby entry are kept, counted from when they were created.
pub(crate) const RETENTION_SECONDS: i64 = 30 * 86_400;

/// Delete what has outlived the retention period and return how many sessions
/// went. A session's members, invites, rounds, snapshots, feedback and its
/// observation, transcript and audio rows go with it, so a session is kept
/// while its room or an audio file may still need cleaning up.
pub(crate) fn prune(accounts: &Accounts, now: i64) -> rusqlite::Result<usize> {
    let cutoff = now.saturating_sub(RETENTION_SECONDS);
    accounts.transaction(|c| {
        c.execute(
            "DELETE FROM peer_pair_requests WHERE status != 'pending' AND created_at < ?1",
            [cutoff],
        )?;
        c.execute(
            "DELETE FROM peer_availability WHERE created_at < ?1
             AND availability_id NOT IN (SELECT availability_id FROM peer_pair_requests)
             AND ((status = 'expired' AND room_deleted_at IS NOT NULL)
                  OR (status = 'paired' AND session_id IN (
                      SELECT id FROM peer_sessions
                      WHERE status IN ('complete','cancelled','finalizing')
                      AND (room_name IS NULL OR room_deleted_at IS NOT NULL))))",
            [cutoff],
        )?;
        c.execute(
            "DELETE FROM peer_sessions WHERE status IN ('complete','cancelled','finalizing')
             AND created_at < ?1
             AND (room_name IS NULL OR room_deleted_at IS NOT NULL)
             AND id NOT IN (SELECT session_id FROM peer_availability WHERE session_id IS NOT NULL)
             AND id NOT IN (SELECT session_id FROM peer_pair_requests WHERE session_id IS NOT NULL)
             AND NOT EXISTS (SELECT 1 FROM peer_audio_records a WHERE a.session_id = peer_sessions.id
                             AND a.purged_at IS NULL AND a.status IN ('recording','ready','deleted'))",
            [cutoff],
        )
    })
}

#[cfg(test)]
#[path = "../../../tests/unit/accounts/peer/retention.rs"]
mod tests;

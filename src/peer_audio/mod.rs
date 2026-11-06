//! Private, explicitly selected peer audio artifacts. These are member-scoped
//! session records, separate from the AI interview's email delivery pipeline.
use crate::accounts::Accounts;
use rusqlite::{OptionalExtension, params};
use std::path::Path;
mod wav;
pub(crate) use wav::{PeerAudioArtifact, PeerAudioWriter, audio_path, ensure_private_directory};
pub(crate) const RETENTION_SECONDS: i64 = 24 * 60 * 60;

pub(crate) fn mark_started(
    accounts: &Accounts,
    id: &str,
    now: i64,
    max_seconds: u64,
) -> rusqlite::Result<()> {
    accounts.with(|c| {
        c.execute("INSERT INTO peer_audio_records(session_id,status,created_at,expires_at) VALUES(?1,'recording',?2,?3) ON CONFLICT(session_id) DO NOTHING",params![id,now,now.saturating_add(i64::try_from(max_seconds).unwrap_or(i64::MAX)).saturating_add(300)])?;
        Ok(())
    })
}

pub(crate) fn mark_ready(
    accounts: &Accounts,
    id: &str,
    artifact: &PeerAudioArtifact,
    now: i64,
) -> rusqlite::Result<()> {
    accounts.with(|c| {
        c.execute("UPDATE peer_audio_records SET status='ready',bytes=?2,duration_ms=?3,expires_at=?4 WHERE session_id=?1 AND status='recording'",params![id,i64::try_from(artifact.bytes).unwrap_or(i64::MAX),i64::try_from(artifact.duration_ms).unwrap_or(i64::MAX),now.saturating_add(RETENTION_SECONDS)])?;
        Ok(())
    })
}

pub(crate) fn mark_failed(accounts: &Accounts, id: &str, now: i64) -> rusqlite::Result<()> {
    accounts.with(|c| {
        c.execute("UPDATE peer_audio_records SET status='failed',expires_at=?2 WHERE session_id=?1 AND status='recording'",params![id,now])?;
        Ok(())
    })
}

pub(crate) struct AudioRecord {
    pub status: String,
    pub bytes: u64,
    pub duration_ms: u64,
    pub expires_at: i64,
    pub purged_at: Option<i64>,
}

pub(crate) fn record(accounts: &Accounts, id: &str) -> rusqlite::Result<Option<AudioRecord>> {
    accounts.with(|c| c.query_row("SELECT status,bytes,duration_ms,expires_at,purged_at FROM peer_audio_records WHERE session_id=?1",[id],|row| Ok(AudioRecord {
        status: row.get(0)?, bytes: u64::try_from(row.get::<_,i64>(1)?).unwrap_or(0), duration_ms: u64::try_from(row.get::<_,i64>(2)?).unwrap_or(0), expires_at: row.get(3)?,
                purged_at: row.get(4)?,
    })).optional())
}

/// Move a completed artifact into deletion before touching its file, so every
/// later read is refused even when unlink needs a retry. The observer closes
/// its writer before the request removes the saved or partial file.
pub(crate) fn withdraw(accounts: &Accounts, id: &str, now: i64) -> rusqlite::Result<()> {
    accounts.with(|c| {
        // A withdrawal before capture begins has no row to update; recording it
        // keeps the observer from starting a capture the member refused.
        c.execute(
            "INSERT INTO peer_audio_records(session_id,status,created_at,expires_at) VALUES(?1,'deleted',?2,?2) ON CONFLICT(session_id) DO NOTHING",
            params![id, now],
        )?;
        c.execute(
            "UPDATE peer_audio_records SET status='deleted',expires_at=?2 WHERE session_id=?1",
            params![id, now],
        )?;
        Ok(())
    })
}

pub(crate) fn sweep(accounts: &Accounts, dir: &Path, now: i64) -> rusqlite::Result<()> {
    let ids = accounts.with(|c| {
        c.prepare(
            "SELECT session_id FROM peer_audio_records WHERE expires_at<=?1 AND purged_at IS NULL",
        )?
        .query_map([now], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()
    })?;
    for id in ids {
        let path = match audio_path(dir, &id) {
            Ok(path) => path,
            Err(_) => continue,
        };
        let mut removed = true;

        // The part file first: a writer finishing in between renames it onto
        // the final name, which the second removal then still catches.
        for file in [&path.with_extension("wav.part"), &path] {
            if let Err(error) = std::fs::remove_file(file)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                removed = false;
            }
        }
        accounts.with(|c| {
            c.execute("UPDATE peer_audio_records SET status='deleted',bytes=0,purged_at=CASE WHEN ?2 THEN ?3 ELSE NULL END WHERE session_id=?1",params![id,removed,now])?;
            Ok(())
        })?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/peer_audio/store.rs"]
mod tests;

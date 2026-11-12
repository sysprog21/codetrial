use super::BackgroundTasks;
use crate::accounts::{Accounts, blocking, peer};
use crate::config::ProviderPool;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;

const SWEEP_INTERVAL: Duration = Duration::from_secs(5);
const RETENTION_INTERVAL: Duration = Duration::from_secs(3600);
const DELETE_TIMEOUT: Duration = Duration::from_secs(3);
const DELETE_CONCURRENCY: usize = 4;

pub(crate) fn spawn_sweeper(accounts: Arc<Accounts>, pool: ProviderPool) -> BackgroundTasks {
    let mut tasks = BackgroundTasks::default();
    let settlements = Arc::clone(&accounts);
    tasks.push("settle peer interviews", async move {
        loop {
            tokio::time::sleep(SWEEP_INTERVAL).await;
            if let Err(error) =
                settle_once(Arc::clone(&settlements), crate::current_epoch_seconds_i64()).await
            {
                eprintln!("peer settlement failed: {error}");
            }
        }
    });
    let retention = Arc::clone(&accounts);
    tasks.push("prune old peer practice", async move {
        // A server restarted more often than the interval would otherwise never
        // reach its first prune.
        loop {
            let accounts = Arc::clone(&retention);
            let now = crate::current_epoch_seconds_i64();
            match blocking(move || peer::retention::prune(&accounts, now)).await {
                Ok(0) => {}
                Ok(removed) => eprintln!("peer retention removed {removed} closed sessions"),
                Err(error) => eprintln!("peer retention failed: {error}"),
            }
            tokio::time::sleep(RETENTION_INTERVAL).await;
        }
    });
    tasks.push("delete closed peer rooms", async move {
        loop {
            tokio::time::sleep(SWEEP_INTERVAL).await;
            if let Err(error) = cleanup_once(
                Arc::clone(&accounts),
                &pool,
                crate::current_epoch_seconds_i64(),
            )
            .await
            {
                eprintln!("peer room cleanup failed: {error}");
            }
        }
    });
    tasks
}

async fn settle_once(accounts: Arc<Accounts>, now: i64) -> rusqlite::Result<()> {
    blocking(move || {
        accounts.transaction(|connection| {
            peer::discovery::settle(connection, now)?;
            let query = "SELECT id FROM peer_sessions
                         WHERE status NOT IN ('cancelled','complete','finalizing')";
            let ids = connection
                .prepare(query)?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for id in ids {
                let Some(mut session) = load_or_skip(connection, &id, "settlement") else {
                    continue;
                };
                let version = session.version;
                peer::settle(connection, &mut session, now)?;
                if session.version != version {
                    peer::save(connection, &session)?;
                }
            }
            Ok(())
        })
    })
    .await
}

enum CleanupTarget {
    Session { id: String, generation: u64 },
    Waiting { id: String },
}

struct CleanupRoom {
    target: CleanupTarget,
    room: String,
    provider: String,
}

/// A session a sweep pass reads, or none when it is gone or cannot be read. One
/// unreadable row is skipped, not fatal: an error here rolls the whole pass
/// back, which would stop every other session from being settled or cleaned up
/// until the row is repaired.
fn load_or_skip(connection: &rusqlite::Connection, id: &str, pass: &str) -> Option<peer::Session> {
    match peer::load(connection, id) {
        Ok(session) => {
            forget_report(pass, id);
            session
        }
        Err(error) => {
            if first_report(pass, id) {
                eprintln!("peer {pass} skipped session {id}: {error}");
            }
            None
        }
    }
}

/// How many unreadable sessions are remembered, so a table of bad rows cannot
/// grow the set without limit.
const REPORT_BOUND: usize = 1024;

/// The sessions each pass has already said it skipped.
static REPORTED: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<(String, String)>>,
> = std::sync::LazyLock::new(Default::default);

/// Whether this pass has not yet said that it skipped this session. A pass runs
/// every few seconds, and a row that cannot be read stays unreadable, so saying
/// it every time fills the log with one line. At the bound nothing new is said
/// and nothing is forgotten: starting over would repeat every bad row on every
/// pass, so the overflow is said once instead.
fn first_report(pass: &str, id: &str) -> bool {
    let mut said = REPORTED.lock().unwrap_or_else(|e| e.into_inner());
    first_report_in(&mut said, REPORT_BOUND, pass, id)
}

/// `first_report` over a set of its own, so a test can fill one to a small
/// bound without touching what the other passes remember.
fn first_report_in(
    said: &mut std::collections::HashSet<(String, String)>,
    bound: usize,
    pass: &str,
    id: &str,
) -> bool {
    let key = (pass.to_owned(), id.to_owned());
    if said.contains(&key) {
        return false;
    }
    if said.len() >= bound {
        if said.insert((pass.to_owned(), String::new())) {
            eprintln!("peer {pass}: over {bound} sessions are unreadable; saying no more");
        }
        return false;
    }
    said.insert(key)
}

/// A session that read cleanly is forgotten, so a later failure of it is said.
fn forget_report(pass: &str, id: &str) {
    REPORTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&(pass.to_owned(), id.to_owned()));
}

fn claim_rooms(accounts: &Accounts, now: i64) -> rusqlite::Result<Vec<CleanupRoom>> {
    let mut rooms = accounts.transaction(|connection| {
        let query = "SELECT id FROM peer_sessions
                         WHERE status IN ('cancelled','complete','finalizing')
                         AND room_name IS NOT NULL AND room_deleted_at IS NULL
                         AND next_delete_at<=?1 LIMIT 16";
        let ids = connection
            .prepare(query)?
            .query_map([now], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut rooms = Vec::new();
        for id in ids {
            let Some(mut session) = load_or_skip(connection, &id, "cleanup") else {
                // Left due, it would be read again by every pass, and sixteen
                // such rows would fill the window and keep every readable
                // closed room out of it. A row that cannot be read does not
                // mend itself, so it is tried again only hourly.
                connection.execute(
                    "UPDATE peer_sessions SET next_delete_at=?2 WHERE id=?1",
                    rusqlite::params![id, now + 3600],
                )?;
                continue;
            };
            if let (Some(room), Some(provider)) = (&session.room_name, &session.provider_id) {
                rooms.push(CleanupRoom {
                    target: CleanupTarget::Session {
                        id,
                        generation: session.room_generation,
                    },
                    room: room.clone(),
                    provider: provider.clone(),
                });
                session.next_delete_at = now + 30;
                peer::save(connection, &session)?;
            }
        }
        Ok(rooms)
    })?;
    rooms.extend(
        peer::discovery::cleanup_candidates(accounts, now)?
            .into_iter()
            .map(|availability| CleanupRoom {
                target: CleanupTarget::Waiting {
                    id: availability.id,
                },
                room: availability.room,
                provider: availability.provider_id,
            }),
    );
    Ok(rooms)
}

async fn cleanup_once(
    accounts: Arc<Accounts>,
    pool: &ProviderPool,
    now: i64,
) -> rusqlite::Result<()> {
    let database = Arc::clone(&accounts);
    let rooms = blocking(move || claim_rooms(&database, now)).await?;
    let results = futures_util::stream::iter(rooms)
        .map(|room| {
            let accounts = Arc::clone(&accounts);
            async move { delete_room(accounts, pool, room, now).await }
        })
        .buffer_unordered(DELETE_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    for result in results {
        result?;
    }
    Ok(())
}

async fn delete_room(
    accounts: Arc<Accounts>,
    pool: &ProviderPool,
    room: CleanupRoom,
    now: i64,
) -> rusqlite::Result<()> {
    let success = match pool.get(&room.provider) {
        Some(provider) => {
            match tokio::time::timeout(
                DELETE_TIMEOUT,
                crate::livekit::delete_room(
                    &provider.url,
                    &provider.api_key,
                    &provider.api_secret,
                    &room.room,
                    u64::try_from(now).unwrap_or(0),
                ),
            )
            .await
            {
                Ok(Ok(())) => true,
                Ok(Err(error)) => {
                    eprintln!("peer room {} was not deleted: {error}", room.room);
                    false
                }
                Err(_) => {
                    eprintln!("peer room {} was not deleted: timed out", room.room);
                    false
                }
            }
        }
        None => {
            eprintln!(
                "peer room {} was not deleted: provider {} is not configured",
                room.room, room.provider
            );
            false
        }
    };
    blocking(move || match room.target {
        CleanupTarget::Waiting { id } => {
            peer::discovery::cleanup_finished(&accounts, &id, success, now)
        }
        CleanupTarget::Session { id, generation } => accounts.transaction(|connection| {
            if let Some(mut session) = peer::load(connection, &id)?
                && session.room_generation == generation
                && session.room_deleted_at.is_none()
            {
                session.delete_attempts = session.delete_attempts.saturating_add(1);
                if success {
                    session.room_deleted_at = Some(now);
                } else {
                    session.next_delete_at =
                        now + 30_i64.saturating_mul(1_i64 << session.delete_attempts.min(6));
                }
                peer::save(connection, &session)?;
            }
            Ok(())
        }),
    })
    .await
}

#[cfg(test)]
async fn sweep_once(
    accounts: Arc<Accounts>,
    pool: &ProviderPool,
    now: i64,
) -> rusqlite::Result<()> {
    settle_once(Arc::clone(&accounts), now).await?;
    cleanup_once(accounts, pool, now).await
}

#[cfg(test)]
#[path = "../../../tests/unit/web/peer/sweeper.rs"]
mod tests;

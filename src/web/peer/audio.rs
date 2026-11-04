use crate::accounts::{Accounts, blocking, peer};
use crate::peer_audio;
use crate::web::{AppState, BackgroundTasks, json_response};
use axum::body::Body;
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use std::sync::Arc;
use tokio::io::AsyncReadExt;

pub(super) async fn route(
    state: AppState,
    accounts: Arc<Accounts>,
    id: String,
    user: i64,
    method: Method,
    status_only: bool,
) -> Response {
    let db = Arc::clone(&accounts);
    let session_id = id.clone();
    let now = crate::current_epoch_seconds_i64();
    let saved = blocking(move || {
        peer::transact(&db, &session_id, user, now, |_, session, _| {
            Ok(Ok(session.settings.save_audio))
        })
    })
    .await;
    match saved {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => return unavailable(status_only),
        Ok(Err(_)) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    let Some(dir) = state
        .config
        .peer_observer
        .as_ref()
        .and_then(|config| config.audio_dir.clone())
    else {
        return unavailable(status_only);
    };

    // A withdrawal needs no existing record: one made before capture starts is
    // recorded, and the observer then declines to record.
    if method == Method::DELETE {
        let db = Arc::clone(&accounts);
        let session_id = id.clone();
        if blocking(move || peer_audio::withdraw(&db, &session_id, now))
            .await
            .is_err()
        {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        if !state.peer_observers.withdraw_audio(&id).await {
            return json_response(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"code":"audio_withdraw_failed","error":"Audio reads are blocked; stopping capture will be retried."}),
            );
        }
        let result = blocking(move || {
            peer_audio::sweep(&accounts, &dir, now)?;
            peer_audio::record(&accounts, &id)
        })
        .await;
        return match result {
            Ok(Some(record)) if record.purged_at.is_some() => {
                json_response(StatusCode::OK, json!({"status":"deleted"}))
            }
            Ok(_) => json_response(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"code":"audio_delete_pending","error":"Audio capture stopped; file deletion will be retried."}),
            ),
            Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
    }
    let db = Arc::clone(&accounts);
    let session_id = id.clone();
    let record = match blocking(move || peer_audio::record(&db, &session_id)).await {
        Ok(Some(record)) => record,
        Ok(None) => return unavailable(status_only),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if status_only {
        let status = if record.expires_at <= now {
            "deleted"
        } else {
            &record.status
        };
        return json_response(
            StatusCode::OK,
            json!({"status":status,"bytes":record.bytes,"durationMs":record.duration_ms,"expiresAt":record.expires_at,"deletionPending":status=="deleted" && record.purged_at.is_none()}),
        );
    }
    if method != Method::GET {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    if record.status != "ready" || record.expires_at <= now {
        return json_response(StatusCode::GONE, json!({"code":"audio_unavailable"}));
    }
    let Ok(path) = peer_audio::audio_path(&dir, &id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(_) => return StatusCode::GONE.into_response(),
    };
    let stream = futures_util::stream::try_unfold(file, |mut file| async move {
        let mut bytes = vec![0; 64 * 1024];
        let count = file.read(&mut bytes).await?;
        if count == 0 {
            return Ok::<_, std::io::Error>(None);
        }
        bytes.truncate(count);
        Ok(Some((bytes, file)))
    });
    (
        [
            (header::CONTENT_TYPE, "audio/wav"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=peer-interview.wav",
            ),
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

fn unavailable(status_only: bool) -> Response {
    if status_only {
        json_response(StatusCode::OK, json!({"status":"unavailable"}))
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

pub(crate) fn spawn_audio_retention(
    accounts: Arc<Accounts>,
    dir: std::path::PathBuf,
) -> BackgroundTasks {
    let mut tasks = BackgroundTasks::default();
    tasks.push("delete expired peer audio", async move {
        loop {
            let db = Arc::clone(&accounts);
            let root = dir.clone();
            if let Err(error) =
                blocking(move || peer_audio::sweep(&db, &root, crate::current_epoch_seconds_i64()))
                    .await
            {
                eprintln!("peer audio retention failed: {error}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
    tasks
}

#[cfg(test)]
#[path = "../../../tests/unit/web/peer/audio.rs"]
mod tests;

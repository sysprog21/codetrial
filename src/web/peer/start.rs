//! Consent is committed before the subscribe-only participant connects. The
//! actual start then checks that no readiness/version changed during the join.
use crate::accounts::{Accounts, blocking, peer};
use crate::web::AppState;
use axum::response::Response;
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug)]
struct Prepared {
    consented: bool,
    version: u64,
    provider: Option<String>,
}

fn prepare(session: &mut peer::Session, slot: usize, body: &Value) -> peer::PeerResult<Prepared> {
    if session.closed() {
        return Err("session_closed");
    }
    if body["expectedVersion"].as_u64() != Some(session.version) {
        return Err("version_conflict");
    }
    if body["observerConsent"].as_bool() != Some(true) {
        return Err("observer_consent_required");
    }
    if session.current_round.is_some() {
        return Err("round_active");
    }
    if session.members.len() != 2
        || session
            .members
            .iter()
            .any(|m| !m.ready || m.entered_at.is_none())
    {
        return Err("waiting_for_partner");
    }
    if !session.observer_consented[slot] {
        session.observer_consented[slot] = true;
        session.version += 1;
    }
    Ok(Prepared {
        consented: session.observer_consented.iter().all(|flag| *flag),
        version: session.version,
        provider: session.provider_id.clone(),
    })
}

pub(super) async fn handle(
    state: AppState,
    accounts: Arc<Accounts>,
    id: String,
    user: i64,
    mut body: Value,
) -> Response {
    let db = Arc::clone(&accounts);
    let session_id = id.clone();
    let proposal = body.clone();
    let now = crate::current_epoch_seconds_i64();
    let prepared = blocking(move || {
        peer::transact(&db, &session_id, user, now, |_, session, slot| {
            Ok(prepare(session, slot, &proposal))
        })
    })
    .await;
    let prepared = match prepared {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(code)) => return super::error(code),
        Err(_) => return super::error("database_unavailable"),
    };

    // Startup refuses peer mode without an observer, so only a router built
    // directly (the integration fixtures) reaches here without one.
    if let (true, Some(config)) = (prepared.consented, state.config.peer_observer.clone()) {
        let Some(provider) = prepared
            .provider
            .as_deref()
            .and_then(|id| state.config.pool.get(id))
            .cloned()
        else {
            return super::error("provider_unavailable");
        };
        if let Err(code) = state
            .peer_observers
            .start_ready(Arc::clone(&accounts), config, id.clone(), provider)
            .await
        {
            return super::error(code);
        }
    }
    body["expectedVersion"] = serde_json::json!(prepared.version);
    let now = crate::current_epoch_seconds_i64();
    super::reply(
        blocking(move || {
            peer::transact(
                &accounts,
                &id,
                user,
                now,
                |c, session, slot| match peer::action(c, session, slot, "start", &body, now)? {
                    Ok(()) => Ok(Ok(session.view(user, now))),
                    Err(code) => Ok(Err(code)),
                },
            )
        })
        .await,
    )
}

#[cfg(test)]
#[path = "../../../tests/unit/web/peer/start.rs"]
mod tests;

//! Authenticated discovery. Availability is an opt-in, never an account list.
use super::{AppState, error, json_response, reply, settings};
use crate::accounts::{Accounts, blocking, peer::discovery as store};
use axum::{
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use std::sync::Arc;

pub(super) async fn route(
    state: AppState,
    accounts: Arc<Accounts>,
    user: i64,
    path: &str,
    method: Method,
    body: Value,
    now: i64,
) -> Response {
    match (path, &method) {
        ("profile", &Method::GET) => {
            reply(blocking(move || store::profile(&accounts, user).map(Ok)).await)
        }
        ("profile", &Method::PUT) => {
            let Some(name) = body["nickname"].as_str().map(str::to_owned) else {
                return error("invalid_nickname");
            };
            reply(blocking(move || store::set_profile(&accounts, user, &name, now)).await)
        }
        ("lobby", &Method::GET) => {
            reply(blocking(move || store::lobby(&accounts, user, now).map(Ok)).await)
        }
        ("availability", &Method::POST) => publish(state, accounts, user, body, now).await,
        ("availability", &Method::DELETE) => {
            reply(blocking(move || store::leave(&accounts, user, now).map(Ok)).await)
        }
        ("availability/heartbeat", &Method::POST) => {
            reply(blocking(move || store::heartbeat(&accounts, user, now)).await)
        }
        ("pairing-requests", &Method::POST) => {
            let Some(id) = body["availabilityId"]
                .as_str()
                .filter(|id| id.len() <= 128)
                .map(str::to_owned)
            else {
                return error("invalid_request");
            };
            reply(blocking(move || store::request(&accounts, user, &id, now)).await)
        }
        _ => {
            let parts = path.split('/').collect::<Vec<_>>();
            if parts.len() != 3 || parts[0] != "pairing-requests" || method != Method::POST {
                return error("not_found");
            };
            let id = parts[1].to_owned();
            match parts[2] {
                "confirm" => {
                    let consent = body["observerConsent"].as_bool().unwrap_or(false);
                    let Some(config) = state.config.peer.clone() else {
                        return error("not_found");
                    };
                    reply(
                        blocking(move || {
                            store::confirm(&accounts, user, &id, consent, &config, now)
                        })
                        .await,
                    )
                }
                "decline" => {
                    reply(blocking(move || store::decline(&accounts, user, &id, now)).await)
                }
                _ => error("not_found"),
            }
        }
    }
}

async fn publish(
    state: AppState,
    accounts: Arc<Accounts>,
    user: i64,
    body: Value,
    now: i64,
) -> Response {
    let Some(config) = state.config.peer.clone() else {
        return error("not_found");
    };
    let settings = match settings::parse(&body, &config) {
        Ok(s) => s,
        Err(code) => return error(code),
    };
    let Some(observer) = state.config.peer_observer.as_ref() else {
        return error("observer_unavailable");
    };
    if settings.save_audio && observer.audio_dir.is_none() {
        return error("audio_unavailable");
    }
    let choice = super::pick_available_provider(&state).await;
    let db = Arc::clone(&accounts);
    let prepared = blocking(move || {
        let placement = choice
            .as_ref()
            .map(|(room, provider_id)| (room.as_str(), provider_id.as_str()));
        store::prepare(&db, user, settings, placement, &config, now)
    })
    .await;
    let a = match prepared {
        Ok(Ok(a)) => a,
        Ok(Err(code)) => return error(code),
        Err(e) => {
            eprintln!("peer availability preparation failed: {e}");
            return error("database_unavailable");
        }
    };
    let Some(provider) = state.config.pool.get(&a.provider_id) else {
        return error("provider_unavailable");
    };
    let needs_creation = a.status != "available" && state.config.provision_rooms;
    let success = if needs_creation {
        match crate::livekit::create_peer_waiting_room(
            &provider.url,
            &provider.api_key,
            &provider.api_secret,
            &a.room,
            u64::try_from(now).unwrap_or(0),
            state
                .config
                .peer
                .as_ref()
                .map_or(120, |p| p.reconnect_grace_seconds),
        )
        .await
        {
            Ok(()) => true,
            Err(e) => {
                eprintln!(
                    "peer waiting room creation failed provider={} error={e}",
                    a.provider_id
                );
                false
            }
        }
    } else {
        true
    };
    let db = Arc::clone(&accounts);
    let id = a.id.clone();
    let published = blocking(move || {
        store::publish(&db, &id, user, success, crate::current_epoch_seconds_i64())
    })
    .await;
    let a = match published {
        Ok(Ok(a)) => a,
        Ok(Err(code)) => return error(code),
        Err(_) => return error("database_unavailable"),
    };
    let input = crate::token::LivekitTokenInput {
        api_key: &provider.api_key,
        api_secret: &provider.api_secret,
        name: "Waiting for a peer",
        identity: &a.identity,
        room: &a.room,
        metadata: "",
        now_seconds: crate::current_epoch_seconds(),
        agent: false,
    };
    let token = match crate::token::livekit_peer_token(input) {
        Ok(token) => token,
        Err(_) => return error("provider_unavailable"),
    };
    json_response(
        StatusCode::OK,
        json!({"availabilityId":a.id,"expiresAt":a.expires_at,"url":provider.url,"room":a.room,"identity":a.identity,"token":token,"settings":a.settings}),
    )
}

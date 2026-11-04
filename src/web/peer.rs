//! Peer routes never enter the AI token dispatcher. SQLite owns admission and
//! rounds; LiveKit carries only the two fixed members' audio and previews.
use super::{AppState, BackgroundTasks, json_response};
use crate::accounts::{
    Accounts, blocking,
    peer::{self, PeerResult, Session},
};
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{Method, Request, StatusCode},
    response::Response,
    routing::any,
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::Ordering};
mod audio;
mod auth;
pub(crate) mod observer;
pub(crate) use audio::spawn_audio_retention;
mod discovery;
mod guide;

mod settings;
mod start;
mod sweeper;
use auth::VerifiedPeer;
pub(crate) use sweeper::spawn_sweeper;

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/api/peer/{*path}", any(handle))
}

fn reply(result: rusqlite::Result<PeerResult<Value>>) -> Response {
    match result {
        Ok(Ok(value)) => json_response(StatusCode::OK, value),
        Ok(Err(code)) => error(code),
        Err(e) => {
            eprintln!("peer database operation failed: {e}");
            error("database_unavailable")
        }
    }
}
fn error(code: &str) -> Response {
    let status = match code {
        "not_found" | "round_unavailable" => StatusCode::NOT_FOUND,
        "invite_unavailable"
        | "session_closed"
        | "availability_unavailable"
        | "request_unavailable" => StatusCode::GONE,
        "at_capacity"
        | "provider_unavailable"
        | "database_unavailable"
        | "observer_unavailable"
        | "observer_at_capacity"
        | "observer_connect_failed"
        | "observer_connect_timeout"
        | "observer_token_unavailable"
        | "audio_unavailable" => StatusCode::SERVICE_UNAVAILABLE,
        "not_writer" | "nickname_required" | "observer_consent_required" => StatusCode::FORBIDDEN,
        "invalid_nickname"
        | "invalid_settings"
        | "invalid_snapshot"
        | "invalid_feedback"
        | "invalid_request"
        | "invalid_action"
        | "problems_unavailable" => StatusCode::BAD_REQUEST,
        _ => StatusCode::CONFLICT,
    };
    json_response(status, json!({"code":code}))
}

async fn handle(
    State(state): State<AppState>,
    VerifiedPeer(owner): VerifiedPeer,
    Path(path): Path<String>,
    request: Request<Body>,
) -> Response {
    let method = request.method().clone();
    let limit = if path.ends_with("/snapshot") {
        super::MAX_PEER_SNAPSHOT_BYTES
    } else if path.ends_with("/feedback") {
        super::MAX_PEER_FEEDBACK_BYTES
    } else {
        super::MAX_BODY_BYTES
    };
    let bytes = match axum::body::to_bytes(request.into_body(), limit).await {
        Ok(b) => b,
        Err(_) => {
            return json_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                json!({"code":"body_too_large"}),
            );
        }
    };
    let body = if bytes.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => v,
            Err(_) => return error("invalid_request"),
        }
    };
    let now = crate::current_epoch_seconds_i64();
    let user = owner.user.id;
    let accounts = owner.accounts;
    if ["profile", "lobby", "availability", "pairing-requests"]
        .contains(&path.split('/').next().unwrap_or(""))
    {
        return discovery::route(state, accounts, user, &path, method, body, now).await;
    }
    let parts = path.split('/').collect::<Vec<_>>();
    if parts == ["sessions"] {
        if method == Method::GET {
            return reply(blocking(move || peer::list(&accounts, user, now).map(Ok)).await);
        }
        if method == Method::POST {
            let Some(config) = state.config.peer.clone() else {
                return error("not_found");
            };
            let settings = match settings::parse(&body, &config) {
                Ok(settings) => settings,
                Err(code) => return error(code),
            };
            if settings.save_audio
                && state
                    .config
                    .peer_observer
                    .as_ref()
                    .is_none_or(|observer| observer.audio_dir.is_none())
            {
                return error("audio_unavailable");
            }
            return reply(
                blocking(move || peer::create(&accounts, user, settings, &config, now)).await,
            );
        }
    }
    if parts == ["invites", "accept"] && method == Method::POST {
        let Some(token) = body["token"]
            .as_str()
            .filter(|t| t.len() <= 128)
            .map(str::to_owned)
        else {
            return error("invite_unavailable");
        };
        return reply(blocking(move || peer::accept(&accounts, user, &token, now)).await);
    }
    if parts.len() < 2 || parts[0] != "sessions" {
        return error("not_found");
    }
    let id = parts[1].to_owned();
    if parts.len() == 2 && method == Method::GET {
        return reply(
            blocking(move || {
                peer::transact(&accounts, &id, user, now, |_, s, _| {
                    Ok(Ok(s.view(user, now)))
                })
            })
            .await,
        );
    }
    if parts.len() == 3 && ["advice", "transcript"].contains(&parts[2]) && method == Method::GET {
        let transcript = parts[2] == "transcript";
        return reply(
            blocking(move || peer::observer::view(&accounts, &id, user, transcript)).await,
        );
    }
    if parts.len() == 3 && parts[2] == "transcript" && method == Method::DELETE {
        return reply(blocking(move || peer::observer::withdraw(&accounts, &id, user, now)).await);
    }
    if parts.len() == 3 && parts[2] == "enter" && method == Method::POST {
        return enter(state, accounts, id, user, now).await;
    }
    if parts.len() == 3 && parts[2] == "token" && method == Method::POST {
        return token(state, accounts, id, user, now).await;
    }
    if parts.get(2) == Some(&"audio")
        && (parts.len() == 3 || (parts.len() == 4 && parts[3] == "status" && method == Method::GET))
    {
        return audio::route(state, accounts, id, user, method, parts.len() == 4).await;
    }
    if parts.len() == 3 && parts[2] == "start" && method == Method::POST {
        return start::handle(state, accounts, id, user, body).await;
    }
    let action = parts.get(2).copied().unwrap_or("").to_owned();
    let tail = parts
        .iter()
        .skip(3)
        .map(|x| (*x).to_owned())
        .collect::<Vec<_>>();
    reply(blocking(move||peer::transact(&accounts,&id,user,now,|c,s,slot|{
        let round_shape=action=="rounds" && tail.len()==2;
        if !round_shape && (action=="rounds" || !tail.is_empty()){return Ok(Err("not_found"));}
        if round_shape{
            let Ok(index)=tail[0].parse::<usize>()else{return Ok(Err("round_unavailable"))};
            return round_route(c,s,slot,index,&tail[1],&method,&body,now);
        }
        if action=="invite" && matches!(method,Method::POST|Method::DELETE){
            if s.creator_id!=user || s.members.len()!=1 || s.status!="pending"{return Ok(Err("invite_unavailable"));}
            if body["expectedVersion"].as_u64()!=Some(s.version){return Ok(Err("version_conflict"));}
            s.version+=1;
            if method==Method::DELETE{c.execute("UPDATE peer_invites SET revoked_at=?2 WHERE session_id=?1 AND revoked_at IS NULL",params![s.id,now])?;return Ok(Ok(s.view(user,now)));}
            let token=peer::invite(c,s,now)?;let mut view=s.view(user,now);view["inviteToken"]=json!(token);return Ok(Ok(view));
        }
        if method!=Method::POST{return Ok(Err("not_found"));}
        match peer::action(c,s,slot,&action,&body,now)?{Ok(())=>Ok(Ok(s.view(user,now))),Err(e)=>Ok(Err(e))}
    })).await)
}

#[expect(
    clippy::too_many_arguments,
    reason = "round routes share one checked transaction and membership"
)]
fn round_route(
    c: &rusqlite::Connection,
    s: &mut Session,
    slot: usize,
    index: usize,
    action: &str,
    method: &Method,
    body: &Value,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    match (action, method) {
        ("writer", &Method::POST) => match body.get("previousLease") {
            None => peer::writer(s, slot, index, now),
            Some(Value::String(previous)) => peer::resume_writer(s, slot, index, previous, now),
            Some(_) => Ok(Err("invalid_request")),
        },
        ("snapshot", &Method::GET) => peer::read_snapshot(c, s, index),
        ("snapshot", &Method::PUT) => peer::snapshot(c, s, slot, index, body, now),
        ("feedback", &Method::PUT) => peer::feedback(c, s, slot, index, body, now),
        ("feedback", &Method::GET) => peer::read_feedback(c, s, slot, index, now),
        ("finish", &Method::POST) => {
            let mut body = body.clone();
            body["roundIndex"] = json!(index);
            match peer::action(c, s, slot, "finish", &body, now)? {
                Ok(()) => Ok(Ok(s.view(s.members[slot].user_id, now))),
                Err(e) => Ok(Err(e)),
            }
        }
        ("problem", &Method::GET) | ("guide", &Method::GET) => {
            if s.current_round != Some(index) {
                return Ok(Err("round_unavailable"));
            }
            let Some(r) = s.rounds.get(index) else {
                return Ok(Err("round_unavailable"));
            };
            if action == "guide" && slot != r.interviewer_slot {
                return Ok(Err("not_found"));
            }
            Ok(guide::present(&r.problem_id, action == "guide").ok_or("round_unavailable"))
        }
        _ => Ok(Err("not_found")),
    }
}

async fn enter(
    state: AppState,
    accounts: Arc<Accounts>,
    id: String,
    user: i64,
    now: i64,
) -> Response {
    let db = Arc::clone(&accounts);
    let lookup = id.clone();
    let binding = blocking(move || {
        peer::transact(&db, &lookup, user, now, |_, s, _| {
            if s.closed() {
                return Ok(Err("session_closed"));
            }
            if s.members.len() != 2 {
                return Ok(Err("waiting_for_partner"));
            }
            Ok(Ok((s.room_name.clone(), s.provider_id.clone())))
        })
    })
    .await;
    let (bound_room, bound_provider) = match binding {
        Ok(Ok(binding)) => binding,
        other => return reply(other.map(|r| r.map(|_| Value::Null))),
    };
    let choice = if let (Some(room), Some(provider_id)) = (bound_room, bound_provider) {
        let Some(provider) = state.config.pool.get(&provider_id) else {
            return error("provider_unavailable");
        };
        if !state
            .provider_quota
            .verdict_for(provider)
            .await
            .is_available()
        {
            return error("provider_unavailable");
        }
        Some((room, provider_id))
    } else {
        pick_available_provider(&state).await
    };
    let Some((room, provider_id)) = choice else {
        return error("provider_unavailable");
    };
    let max = state.config.peer.as_ref().map_or(0, |p| p.max_sessions);
    reply(
        blocking(move || {
            peer::transact(&accounts, &id, user, now, |c, s, slot| {
                if s.closed() {
                    return Ok(Err("session_closed"));
                }
                if s.provider_id
                    .as_ref()
                    .is_some_and(|bound| bound != &provider_id)
                {
                    return Ok(Err("version_conflict"));
                }
                let active: Option<String> = c
                    .query_row(
                        "SELECT session_id FROM peer_active_users WHERE user_id=?1",
                        [user],
                        |r| r.get(0),
                    )
                    .optional()?;
                if active.as_ref().is_some_and(|active| active != &id) {
                    return Ok(Err("already_active"));
                }

                let (count, occupied) = peer::room_reservations(c, Some(&id))?;
                if !occupied && count >= i64::try_from(max).unwrap_or(i64::MAX) {
                    return Ok(Err("at_capacity"));
                }
                c.execute(
                    "INSERT OR IGNORE INTO peer_active_users(user_id,session_id) VALUES(?1,?2)",
                    params![user, id],
                )?;
                if s.room_name.is_none() {
                    s.room_name = Some(room);
                    s.provider_id = Some(provider_id);
                    s.room_generation += 1;
                }
                s.members[slot].entered_at = Some(now);
                s.members[slot].last_seen_at = Some(now);
                if s.current_round.is_none() {
                    s.status = "lobby".into();
                }
                s.version += 1;
                Ok(Ok(s.view(user, now)))
            })
        })
        .await,
    )
}

async fn token(
    state: AppState,
    accounts: Arc<Accounts>,
    id: String,
    user: i64,
    now: i64,
) -> Response {
    let config = Arc::clone(&state.config);
    reply(blocking(move||peer::transact(&accounts,&id,user,now,|_,s,slot|{
        if s.closed(){return Ok(Err("session_closed"));}
        if s.members.len()!=2 || s.members.iter().any(|m|m.entered_at.is_none()){return Ok(Err("waiting_for_partner"));}
        let Some(provider)=s.provider_id.as_deref().and_then(|id|config.pool.get(id))else{return Ok(Err("provider_unavailable"))};
        let Some(room)=s.room_name.as_deref()else{return Ok(Err("waiting_for_partner"))};
        let member=&s.members[slot];
        let input=crate::token::LivekitTokenInput{api_key:&provider.api_key,api_secret:&provider.api_secret,name:&member.login,identity:&member.identity,room,metadata:"",now_seconds:u64::try_from(now).unwrap_or(0),agent:false};
        match crate::token::livekit_peer_token(input){Ok(token)=>Ok(Ok(json!({"url":provider.url,"token":token,"room":room,"identity":member.identity}))),Err(_)=>Ok(Err("provider_unavailable"))}
    })).await)
}

/// The next pool provider, in rotation, whose quota still admits a room.
pub(super) async fn pick_available_provider(state: &AppState) -> Option<(String, String)> {
    for _ in 0..state.config.pool.providers.len() {
        let counter = state.provider_counter.fetch_add(1, Ordering::Relaxed);
        let (room, provider) = super::pool::room_and_provider(&state.config, counter);
        if let Some(provider) = provider
            && state
                .provider_quota
                .verdict_for(provider)
                .await
                .is_available()
        {
            return Some((room, provider.id.clone()));
        }
    }
    None
}

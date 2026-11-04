//! Explicit, short-lived availability and mutually confirmed pairing proposals.
//! Waiting rooms are published only after provisioning has succeeded.
use super::{Accounts, PeerResult, Settings, accept_on, create_on, entropy, load, save};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

pub(crate) const AVAILABILITY_TTL_SECONDS: i64 = 45;
const REQUEST_TTL_SECONDS: i64 = 120;

#[derive(Clone)]
pub(crate) struct Availability {
    pub id: String,
    pub user: i64,
    pub status: String,
    pub settings: Settings,
    pub provider_id: String,
    pub room: String,
    pub identity: String,
    pub expires_at: i64,
}

fn availability(c: &Connection, id: &str) -> rusqlite::Result<Option<Availability>> {
    c.query_row(
        "SELECT availability_id, user_id, status, settings_json, provider_id,
             room_name, participant_identity, expires_at FROM peer_availability WHERE
             availability_id=?1",
        [id],
        |row| {
            let settings: String = row.get(3)?;
            Ok(Availability {
                id: row.get(0)?,
                user: row.get(1)?,
                status: row.get(2)?,
                settings: serde_json::from_str(&settings)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                provider_id: row.get(4)?,
                room: row.get(5)?,
                identity: row.get(6)?,
                expires_at: row.get(7)?,
            })
        },
    )
    .optional()
}

pub(crate) fn settle(c: &Connection, now: i64) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE peer_availability SET status='expired' WHERE status IN ('provisioning',
             'available') AND (expires_at<=?1 OR user_id IN (SELECT user_id FROM peer_active_users))",
        [now],
    )?;
    c.execute(
        "UPDATE peer_pair_requests SET status='expired' WHERE status='pending' AND
             (expires_at<=?1 OR availability_id IN (SELECT availability_id FROM
             peer_availability WHERE status!='available'))",
        [now],
    )?;
    Ok(())
}

fn busy(c: &Connection, user: i64) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM peer_active_users WHERE user_id=?1) OR
             EXISTS(SELECT 1 FROM peer_pair_requests r JOIN peer_sessions s ON
             s.id=r.session_id WHERE r.status='paired' AND (r.from_user_id=?1 OR
             r.to_user_id=?1) AND s.status NOT IN ('cancelled', 'complete', 'finalizing'))",
        [user],
        |row| row.get(0),
    )
}

pub(crate) fn profile(accounts: &Accounts, user: i64) -> rusqlite::Result<Value> {
    accounts.with(|c| Ok(json!({"nickname":nickname(c,user)?})))
}

pub(super) fn nickname(c: &Connection, user: i64) -> rusqlite::Result<Option<String>> {
    c.query_row(
        "SELECT nickname FROM peer_profiles WHERE user_id=?1",
        [user],
        |row| row.get(0),
    )
    .optional()
}

pub(crate) fn set_profile(
    accounts: &Accounts,
    user: i64,
    name: &str,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    let name = name.trim();
    if name.is_empty()
        || name.len() > 24
        || !name.bytes().all(|b| b.is_ascii_alphanumeric())
        || !name.bytes().any(|b| b.is_ascii_alphabetic())
    {
        return Ok(Err("invalid_nickname"));
    }
    accounts.transaction(|c| {
        c.execute(
            "INSERT INTO peer_profiles(user_id, nickname, updated_at) VALUES(?1, ?2, ?3) ON
             CONFLICT(user_id) DO UPDATE SET nickname=excluded.nickname,
             updated_at=excluded.updated_at",
            params![user, name, now],
        )?;
        Ok(Ok(json!({"nickname":name})))
    })
}

pub(crate) fn prepare(
    accounts: &Accounts,
    user: i64,
    settings: Settings,
    placement: Option<(&str, &str)>,
    config: &crate::config::PeerConfig,
    now: i64,
) -> rusqlite::Result<PeerResult<Availability>> {
    if !super::valid_settings(&settings, config) {
        return Ok(Err("invalid_settings"));
    }
    accounts.transaction(|c| {
        settle(c, now)?;
        if nickname(c, user)?.is_none() {
            return Ok(Err("nickname_required"));
        }
        if busy(c, user)? {
            return Ok(Err("already_active"));
        }
        let existing: Option<String> = c
            .query_row(
                "SELECT availability_id FROM peer_availability WHERE user_id=?1 AND status IN
             ('provisioning', 'available')",
                [user],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            let existing = availability(c, &id)?.ok_or(rusqlite::Error::InvalidQuery)?;
            if existing.settings != settings {
                return Ok(Err("settings_immutable"));
            }
            if existing.status == "provisioning" {
                return Ok(Err("availability_pending"));
            }
            c.execute(
                "UPDATE peer_availability SET expires_at=?2 WHERE availability_id=?1",
                params![id, now + AVAILABILITY_TTL_SECONDS],
            )?;
            return Ok(Ok(Availability {
                expires_at: now + AVAILABILITY_TTL_SECONDS,
                ..existing
            }));
        }
        let (count, _) = super::room_reservations(c, None)?;
        if count >= i64::try_from(config.max_sessions).unwrap_or(i64::MAX) {
            return Ok(Err("at_capacity"));
        }

        // Only a new waiting room needs a provider; refreshing an available
        // entry above must not depend on one being free right now.
        let Some((room, provider)) = placement else {
            return Ok(Err("provider_unavailable"));
        };
        let a = Availability {
            id: entropy()?,
            user,
            status: "provisioning".into(),
            settings,
            provider_id: provider.into(),
            room: room.into(),
            identity: entropy()?,
            expires_at: now + AVAILABILITY_TTL_SECONDS,
        };
        c.execute(
            "INSERT INTO peer_availability(availability_id, user_id, status, settings_json,
             provider_id, room_name, participant_identity, created_at, expires_at)
             VALUES(?1, ?2, 'provisioning', ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                a.id,
                user,
                serde_json::to_string(&a.settings).expect("settings serialize"),
                a.provider_id,
                a.room,
                a.identity,
                now,
                a.expires_at
            ],
        )?;
        Ok(Ok(a))
    })
}

pub(crate) fn publish(
    accounts: &Accounts,
    id: &str,
    user: i64,
    success: bool,
    now: i64,
) -> rusqlite::Result<PeerResult<Availability>> {
    accounts.transaction(|c| {
        settle(c, now)?;
        let Some(mut a) = availability(c, id)? else {
            return Ok(Err("availability_unavailable"));
        };
        if a.user != user || !["provisioning", "available"].contains(&a.status.as_str()) {
            return Ok(Err("availability_unavailable"));
        }
        if !success {
            c.execute(
                "UPDATE peer_availability SET status='expired' WHERE availability_id=?1",
                [id],
            )?;
            return Ok(Err("provider_unavailable"));
        }
        a.status = "available".into();
        a.expires_at = now + AVAILABILITY_TTL_SECONDS;
        c.execute(
            "UPDATE peer_availability SET status='available', expires_at=?2 WHERE
             availability_id=?1",
            params![id, a.expires_at],
        )?;
        Ok(Ok(a))
    })
}

pub(crate) fn heartbeat(
    accounts: &Accounts,
    user: i64,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    accounts.transaction(|c| {
        settle(c, now)?;
        let count = c.execute(
            "UPDATE peer_availability SET expires_at=?2 WHERE user_id=?1 AND
             status='available'",
            params![user, now + AVAILABILITY_TTL_SECONDS],
        )?;
        if count == 0 {
            return Ok(Err("availability_unavailable"));
        }
        Ok(Ok(
            json!({"available":true,"expiresAt":now+AVAILABILITY_TTL_SECONDS}),
        ))
    })
}

pub(crate) fn leave(accounts: &Accounts, user: i64, now: i64) -> rusqlite::Result<Value> {
    accounts.transaction(|connection| {
        connection.execute(
            "UPDATE peer_availability SET status='expired' WHERE user_id=?1 AND status IN
             ('provisioning', 'available')",
            [user],
        )?;
        connection.execute(
            "UPDATE peer_pair_requests SET status='expired' WHERE status='pending' AND
             (from_user_id=?1 OR to_user_id=?1)",
            [user],
        )?;
        settle(connection, now)?;
        Ok(json!({"available":false}))
    })
}

pub(crate) fn request(
    accounts: &Accounts,
    user: i64,
    id: &str,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    accounts.transaction(|c| {
        settle(c, now)?;
        if nickname(c, user)?.is_none() {
            return Ok(Err("nickname_required"));
        }
        if busy(c, user)? {
            return Ok(Err("already_active"));
        }
        let Some(a) = availability(c, id)? else {
            return Ok(Err("availability_unavailable"));
        };
        if a.status != "available" || a.user == user || busy(c, a.user)? {
            return Ok(Err("availability_unavailable"));
        }
        let outgoing: Option<(String, String)> = c
            .query_row(
                "SELECT request_id, availability_id FROM peer_pair_requests WHERE from_user_id=?1 AND
             status='pending'",
                [user],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((outgoing, target)) = outgoing {
            if target != id { return Ok(Err("request_pending")); }
            return request_view(c, &outgoing, user).map(|v| v.ok_or("request_unavailable"));
        }
        let incoming: i64 = c.query_row(
            "SELECT count(*) FROM peer_pair_requests WHERE to_user_id=?1 AND
             status='pending'",
            [a.user],
            |row| row.get(0),
        )?;
        if incoming >= 8 {
            return Ok(Err("request_limit"));
        }
        let request = entropy()?;
        c.execute(
            "INSERT INTO peer_pair_requests(request_id, availability_id, from_user_id,
             to_user_id, status, settings_json, created_at, expires_at) VALUES(?1, ?2, ?3,
             ?4, 'pending', ?5, ?6, ?7)",
            params![
                request,
                id,
                user,
                a.user,
                serde_json::to_string(&a.settings).expect("settings serialize"),
                now,
                now + REQUEST_TTL_SECONDS
            ],
        )?;
        request_view(c, &request, user).map(|v| v.ok_or("request_unavailable"))
    })
}

fn request_view(c: &Connection, id: &str, user: i64) -> rusqlite::Result<Option<Value>> {
    c.query_row("SELECT r.request_id, r.from_user_id, r.to_user_id, r.status,
             r.settings_json, r.from_confirmed_at, r.to_confirmed_at, r.session_id,
             f.login, t.login, fp.nickname, tp.nickname FROM peer_pair_requests r JOIN
             users f ON f.id=r.from_user_id JOIN users t ON t.id=r.to_user_id LEFT JOIN
             peer_profiles fp ON fp.user_id=f.id LEFT JOIN peer_profiles tp ON
             tp.user_id=t.id WHERE r.request_id=?1 AND (r.from_user_id=?2 OR
             r.to_user_id=?2)",params![id,user],|row|{
        let from:i64=row.get(1)?;let to:i64=row.get(2)?;
        let from_confirmed:Option<i64>=row.get(5)?;let to_confirmed:Option<i64>=row.get(6)?;
        let settings:String=row.get(4)?;
        Ok(json!({
            "requestId":row.get::<_,String>(0)?,
            "status":row.get::<_,String>(3)?,
            "settings":serde_json::from_str::<Value>(&settings).map_err(|_|rusqlite::Error::InvalidQuery)?,
            "confirmedByYou":if from==user{from_confirmed.is_some()}else{to_confirmed.is_some()},
            "confirmedByPartner":if to==user{from_confirmed.is_some()}else{to_confirmed.is_some()},
            "sessionId":row.get::<_,Option<String>>(7)?,
            "fromLogin":row.get::<_,String>(8)?,
            "toLogin":row.get::<_,String>(9)?,
            "fromNickname":row.get::<_,Option<String>>(10)?,
            "toNickname":row.get::<_,Option<String>>(11)?
        }))
    }).optional()
}

pub(crate) fn confirm(
    accounts: &Accounts,
    user: i64,
    id: &str,
    consent: bool,
    config: &crate::config::PeerConfig,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    accounts.transaction(|c| {
        settle(c, now)?;
        let Some(view) = request_view(c, id, user)? else {
            return Ok(Err("not_found"));
        };
        if !consent {
            return Ok(Err("observer_consent_required"));
        }
        if view["status"] == "paired" {
            return Ok(Ok(view));
        }
        if view["status"] != "pending" {
            return Ok(Err("request_unavailable"));
        }
        let (availability_id, from, to): (String, i64, i64) = c.query_row(
            "SELECT availability_id, from_user_id, to_user_id FROM peer_pair_requests WHERE
             request_id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if busy(c, from)? || busy(c, to)? {
            return Ok(Err("already_active"));
        }
        let Some(a) = availability(c, &availability_id)? else {
            return Ok(Err("availability_unavailable"));
        };
        if a.status != "available" {
            return Ok(Err("availability_unavailable"));
        }
        let column = if user == from {
            "from_confirmed_at"
        } else {
            "to_confirmed_at"
        };
        c.execute(
            &format!("UPDATE peer_pair_requests SET {column}=?2 WHERE request_id=?1"),
            params![id, now],
        )?;
        let both: bool = c.query_row(
            "SELECT from_confirmed_at IS NOT NULL AND to_confirmed_at IS NOT NULL FROM
             peer_pair_requests WHERE request_id=?1",
            [id],
            |row| row.get(0),
        )?;
        if both {
            let settings: Settings = serde_json::from_value(view["settings"].clone())
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            let created = match create_on(c, to, settings, config, now)? {
                Ok(created) => created,
                Err(code) => return Ok(Err(code)),
            };
            let session_id = created["sessionId"]
                .as_str()
                .ok_or(rusqlite::Error::InvalidQuery)?;
            let invite = created["inviteToken"]
                .as_str()
                .ok_or(rusqlite::Error::InvalidQuery)?;
            if accept_on(c, from, invite, now)?.is_err() {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let mut session = load(c, session_id)?.ok_or(rusqlite::Error::InvalidQuery)?;
            session.discovery_pair = true;
            session.provider_id = Some(a.provider_id.clone());
            session.room_name = Some(a.room.clone());
            session.room_generation = 1;
            session.members[0].identity = a.identity;
            session.observer_consented = [true, true];
            session.version += 1;
            save(c, &session)?;
            c.execute(
                "UPDATE peer_pair_requests SET status='paired', session_id=?2 WHERE
             request_id=?1",
                params![id, session_id],
            )?;
            c.execute(
                "UPDATE peer_pair_requests SET status='declined' WHERE status='pending' AND
             (from_user_id IN (?1, ?2) OR to_user_id IN (?1, ?2))",
                params![from, to],
            )?;
            c.execute(
                "UPDATE peer_availability SET status='paired', session_id=?2 WHERE
             availability_id=?1",
                params![availability_id, session_id],
            )?;
            c.execute(
                "UPDATE peer_availability SET status='expired' WHERE user_id=?1 AND status IN
             ('provisioning', 'available')",
                [from],
            )?;
        }
        request_view(c, id, user).map(|v| v.ok_or("request_unavailable"))
    })
}

pub(crate) fn decline(
    accounts: &Accounts,
    user: i64,
    id: &str,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    accounts.transaction(|c| {
        settle(c, now)?;
        let Some(view) = request_view(c, id, user)? else {
            return Ok(Err("not_found"));
        };
        if view["status"] != "pending" {
            return Ok(Err("request_unavailable"));
        }
        c.execute(
            "UPDATE peer_pair_requests SET status='declined' WHERE request_id=?1",
            [id],
        )?;
        request_view(c, id, user).map(|v| v.ok_or("request_unavailable"))
    })
}

pub(crate) fn lobby(accounts: &Accounts, user: i64, now: i64) -> rusqlite::Result<Value> {
    accounts.transaction(|c|{
        settle(c,now)?;
        let rows=c.prepare("SELECT a.availability_id, u.login, a.settings_json, p.nickname FROM
             peer_availability a JOIN users u ON u.id=a.user_id JOIN peer_profiles p ON
             p.user_id=a.user_id WHERE a.status='available' AND a.user_id!=?1 AND
             u.github_id>0 ORDER BY a.created_at LIMIT 100")?.query_map([user],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let available=rows.into_iter().map(|(id,login,settings,nickname)|json!({"availabilityId":id,"login":login,"nickname":nickname,"settings":serde_json::from_str::<Value>(&settings).unwrap_or(Value::Null)})).collect::<Vec<_>>();
        let own:Option<(String,String)>=c.query_row("SELECT availability_id, settings_json FROM peer_availability WHERE user_id=?1
             AND status IN ('available', 'provisioning')",[user],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        let session_id:Option<String>=c.query_row("SELECT r.session_id FROM peer_pair_requests r JOIN peer_sessions s ON
             s.id=r.session_id WHERE (r.from_user_id=?1 OR r.to_user_id=?1) AND
             r.status='paired' AND s.status NOT IN ('cancelled', 'complete', 'finalizing')
             ORDER BY r.created_at DESC LIMIT 1",[user],|row|row.get(0)).optional()?;
        let ids=c.prepare("SELECT request_id FROM peer_pair_requests WHERE from_user_id=?1 OR
             to_user_id=?1 ORDER BY created_at DESC LIMIT 50")?.query_map([user],|row|row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut requests=vec![];for id in ids{if let Some(view)=request_view(c,&id,user)?{requests.push(view);}}
        Ok(json!({"available":available,"requests":requests,"you":{"availabilityId":own.as_ref().map(|(id,_)|id),"settings":own.as_ref().and_then(|(_,settings)|serde_json::from_str::<Value>(settings).ok()),"sessionId":session_id,"nickname":nickname(c,user)?},"serverNow":now}))
    })
}

pub(crate) fn cleanup_candidates(
    accounts: &Accounts,
    now: i64,
) -> rusqlite::Result<Vec<Availability>> {
    accounts.transaction(|c| {
        settle(c, now)?;
        let ids = c
            .prepare(
                "SELECT availability_id FROM peer_availability WHERE status='expired' AND
             room_deleted_at IS NULL AND next_delete_at<=?1 LIMIT 16",
            )?
            .query_map([now], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut rooms = vec![];
        for id in ids {
            if let Some(a) = availability(c, &id)? {
                c.execute(
                    "UPDATE peer_availability SET next_delete_at=?2 WHERE availability_id=?1",
                    params![id, now + 30],
                )?;
                rooms.push(a);
            }
        }
        Ok(rooms)
    })
}

pub(crate) fn cleanup_finished(
    accounts: &Accounts,
    id: &str,
    success: bool,
    now: i64,
) -> rusqlite::Result<()> {
    accounts.transaction(|c| {
        c.execute(
            "UPDATE peer_availability SET delete_attempts=delete_attempts+1,
             room_deleted_at=CASE WHEN ?2 THEN ?3 ELSE NULL END, next_delete_at=?3+60 WHERE
             availability_id=?1 AND status='expired' AND room_deleted_at IS NULL",
            params![id, success, now],
        )?;
        Ok(())
    })
}

#[cfg(test)]
#[path = "../../../tests/unit/accounts/peer/discovery.rs"]
mod tests;

//! Transactional authority for reciprocal interviews. HTTP and the sweeper
//! use the same settlement function and persist before returning state.

use super::{Accounts, random_token, session_key};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) mod discovery;
pub(crate) mod observer;
pub(crate) mod retention;
mod rounds;
mod workspace;
pub(crate) use workspace::{
    feedback, read_feedback, read_snapshot, resume_writer, snapshot, writer,
};

pub(crate) type PeerResult<T> = Result<T, &'static str>;

/// Count admission reservations and whether this session already owns one.
pub(crate) fn room_reservations(
    connection: &Connection,
    session_id: Option<&str>,
) -> rusqlite::Result<(i64, bool)> {
    connection.query_row(
        "SELECT count(*), COALESCE(MAX(id=?1),0) FROM (
             SELECT id FROM peer_sessions WHERE room_name IS NOT NULL
             AND status NOT IN ('cancelled','complete','finalizing')
             AND (status!='pending' OR json_extract(state_json,'$.discovery_pair')=1)
             UNION ALL
             SELECT NULL FROM peer_availability WHERE status IN ('provisioning','available')
         )",
        [session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Settings {
    #[serde(default)]
    pub topic: String,
    pub difficulty: String,
    pub language: String,
    pub round_minutes: u32,
    pub interviewer_first: usize,
    #[serde(default)]
    pub save_transcript: bool,
    #[serde(default)]
    pub save_audio: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Member {
    pub slot: usize,
    pub user_id: i64,
    pub login: String,
    pub nickname: Option<String>,
    pub identity: String,
    pub entered_at: Option<i64>,
    pub last_seen_at: Option<i64>,
    pub ready: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Round {
    pub index: usize,
    pub interviewer_slot: usize,
    pub candidate_slot: usize,
    pub problem_id: String,
    pub status: String,
    pub version: u64,
    pub started_at: Option<i64>,
    pub deadline: Option<i64>,
    pub finishing_at: Option<i64>,
    pub sealed_at: Option<i64>,
    pub writer_lease: Option<String>,
    pub writer_lease_until: i64,
    pub final_capture_incomplete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Session {
    pub id: String,
    pub creator_id: i64,
    #[serde(default)]
    pub discovery_pair: bool,
    pub status: String,
    pub version: u64,
    pub settings: Settings,
    pub created_at: i64,
    pub expires_at: i64,
    pub hard_minutes: u32,
    pub reconnect_grace_seconds: u64,
    pub hard_deadline: Option<i64>,
    pub outcome: Option<String>,
    pub provider_id: Option<String>,
    pub room_name: Option<String>,
    pub start_confirmed: [bool; 2],
    #[serde(default)]
    pub observer_consented: [bool; 2],
    pub swap_confirmed: [bool; 2],
    pub current_round: Option<usize>,
    pub room_generation: u64,
    pub room_deleted_at: Option<i64>,
    pub delete_attempts: u32,
    pub next_delete_at: i64,
    #[serde(skip)]
    pub members: Vec<Member>,
    #[serde(skip)]
    pub rounds: Vec<Round>,
}

/// A round index as the narrow integer the tables store it in.
pub(crate) fn round_idx(index: usize) -> rusqlite::Result<u8> {
    u8::try_from(index).map_err(|_| rusqlite::Error::InvalidQuery)
}

impl Round {
    /// Code may still be saved: while it runs, and while its final capture is
    /// awaited.
    pub(crate) fn accepts_writes(&self) -> bool {
        matches!(self.status.as_str(), "active" | "finishing")
    }
}

pub(crate) fn entropy() -> rusqlite::Result<String> {
    random_token(32).map_err(|_| rusqlite::Error::InvalidQuery)
}

pub(crate) fn load(connection: &Connection, id: &str) -> rusqlite::Result<Option<Session>> {
    let data: Option<String> = connection
        .query_row(
            "SELECT state_json FROM peer_sessions WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(data) = data else { return Ok(None) };
    let mut session: Session =
        serde_json::from_str(&data).map_err(|_| rusqlite::Error::InvalidQuery)?;
    session.members = connection.prepare("SELECT slot,peer_members.user_id,participant_identity,entered_at,last_seen_at,users.login,peer_profiles.nickname FROM peer_members JOIN users ON users.id=peer_members.user_id LEFT JOIN peer_profiles ON peer_profiles.user_id=users.id WHERE session_id=?1 ORDER BY slot")?.query_map([id], |row| {
        let slot = usize::from(row.get::<_, u8>(0)?);
        Ok(Member { slot, user_id: row.get(1)?, identity: row.get(2)?, entered_at: row.get(3)?, last_seen_at: row.get(4)?, login: row.get(5)?, nickname:row.get(6)?, ready: false })
    })?.collect::<rusqlite::Result<Vec<_>>>()?;

    // Readiness changes belong to the session proposal and are persisted in
    // that proposal rather than inferred from a live connection.
    let data: Value = serde_json::from_str(&data).map_err(|_| rusqlite::Error::InvalidQuery)?;
    for member in &mut session.members {
        member.ready = data["ready"][member.slot].as_bool().unwrap_or(false);
    }
    session.rounds = connection
        .prepare("SELECT state_json FROM peer_rounds WHERE session_id=?1 ORDER BY idx")?
        .query_map([id], |row| row.get::<_, String>(0))?
        .map(|row| serde_json::from_str(&row?).map_err(|_| rusqlite::Error::InvalidQuery))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(session))
}

pub(crate) fn save(connection: &Connection, session: &Session) -> rusqlite::Result<()> {
    let mut state = serde_json::to_value(session).map_err(|_| rusqlite::Error::InvalidQuery)?;
    state["ready"] = json!([
        session.members.first().is_some_and(|m| m.ready),
        session.members.get(1).is_some_and(|m| m.ready)
    ]);
    connection.execute("INSERT INTO peer_sessions (id,creator_id,status,version,settings_json,state_json,created_at,expires_at,provider_id,room_name,room_generation,hard_deadline,outcome,room_deleted_at,delete_attempts,next_delete_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16) ON CONFLICT(id) DO UPDATE SET status=excluded.status,version=excluded.version,state_json=excluded.state_json,provider_id=excluded.provider_id,room_name=excluded.room_name,room_generation=excluded.room_generation,hard_deadline=excluded.hard_deadline,outcome=excluded.outcome,room_deleted_at=excluded.room_deleted_at,delete_attempts=excluded.delete_attempts,next_delete_at=excluded.next_delete_at", params![session.id,session.creator_id,session.status,i64::try_from(session.version).map_err(|_| rusqlite::Error::InvalidQuery)?,session.settings_json(),state.to_string(),session.created_at,session.expires_at,session.provider_id,session.room_name,i64::try_from(session.room_generation).map_err(|_| rusqlite::Error::InvalidQuery)?,session.hard_deadline,session.outcome,session.room_deleted_at,session.delete_attempts,session.next_delete_at])?;
    for member in &session.members {
        connection.execute("INSERT INTO peer_members (session_id,slot,user_id,participant_identity,entered_at,last_seen_at) VALUES (?1,?2,?3,?4,?5,?6) ON CONFLICT(session_id,slot) DO UPDATE SET participant_identity=excluded.participant_identity,entered_at=excluded.entered_at,last_seen_at=excluded.last_seen_at",params![session.id,u8::try_from(member.slot).map_err(|_| rusqlite::Error::InvalidQuery)?,member.user_id,member.identity,member.entered_at,member.last_seen_at])?;
    }
    for round in &session.rounds {
        connection.execute("INSERT INTO peer_rounds (session_id,idx,interviewer_slot,candidate_slot,problem_id,status,version,started_at,deadline,sealed_at,writer_lease,writer_lease_until,final_capture_incomplete,state_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) ON CONFLICT(session_id,idx) DO UPDATE SET status=excluded.status,version=excluded.version,started_at=excluded.started_at,deadline=excluded.deadline,sealed_at=excluded.sealed_at,writer_lease=excluded.writer_lease,writer_lease_until=excluded.writer_lease_until,final_capture_incomplete=excluded.final_capture_incomplete,state_json=excluded.state_json", params![session.id,u8::try_from(round.index).map_err(|_| rusqlite::Error::InvalidQuery)?,u8::try_from(round.interviewer_slot).map_err(|_| rusqlite::Error::InvalidQuery)?,u8::try_from(round.candidate_slot).map_err(|_| rusqlite::Error::InvalidQuery)?,round.problem_id,round.status,i64::try_from(round.version).map_err(|_| rusqlite::Error::InvalidQuery)?,round.started_at,round.deadline,round.sealed_at,round.writer_lease,round.writer_lease_until,round.final_capture_incomplete,serde_json::to_string(round).map_err(|_| rusqlite::Error::InvalidQuery)?])?;
    }
    Ok(())
}

impl Session {
    fn settings_json(&self) -> String {
        serde_json::to_string(&self.settings).expect("settings contain only JSON scalars")
    }
    pub(crate) fn slot(&self, user: i64) -> Option<usize> {
        self.members
            .iter()
            .find(|m| m.user_id == user)
            .map(|m| m.slot)
    }
    pub(crate) fn closed(&self) -> bool {
        matches!(
            self.status.as_str(),
            "cancelled" | "complete" | "finalizing"
        )
    }
    pub(crate) fn view(&self, user: i64, now: i64) -> Value {
        let slot = self
            .slot(user)
            .expect("membership checked before presentation");
        let round=self.current_round.and_then(|index|self.rounds.get(index)).map(|r|json!({"index":r.index,"status":r.status,"role":if r.candidate_slot==slot {"candidate"}else{"interviewer"},"deadline":r.deadline,"startedAt":r.started_at,"finalCaptureIncomplete":r.final_capture_incomplete}));
        json!({"sessionId":self.id,"status":self.status,"version":self.version,"serverNow":now,"settings":self.settings,"members":self.members.iter().map(|m|json!({"slot":m.slot,"login":m.login,"nickname":m.nickname.as_deref().unwrap_or(&m.login),"identity":m.identity,"entered":m.entered_at.is_some(),"ready":m.ready,"connected":m.entered_at.is_some() && m.last_seen_at.is_some_and(|seen|now.saturating_sub(seen) < i64::try_from(self.reconnect_grace_seconds).unwrap_or(i64::MAX))})).collect::<Vec<_>>(),"you":{"slot":slot,"observerConsented":self.observer_consented[slot]},"currentRound":round,"hardDeadline":self.hard_deadline,"outcome":self.outcome})
    }
}

pub(crate) fn settle(connection: &Connection, s: &mut Session, now: i64) -> rusqlite::Result<()> {
    let before = serde_json::to_string(&(&*s, &s.rounds)).expect("session serializes");
    if s.status == "pending"
        && (now >= s.expires_at || (s.discovery_pair && now >= s.created_at + 600))
    {
        s.status = "cancelled".into();
        s.outcome = Some("no_show".into());
    }
    if s.status == "lobby"
        && s.members
            .iter()
            .filter_map(|m| m.last_seen_at)
            .max()
            .is_some_and(|last| now - last >= 600)
    {
        s.status = "pending".into();
        for m in &mut s.members {
            m.entered_at = None;
            m.ready = false;
        }
        s.start_confirmed = [false; 2];
    }
    if s.hard_deadline.is_some_and(|deadline| now >= deadline) && !s.closed() {
        s.status = "finalizing".into();
        s.outcome = Some("partial".into());
        for r in &mut s.rounds {
            if r.sealed_at.is_none() {
                r.status = "feedback".into();
                r.sealed_at = Some(now);
                r.final_capture_incomplete = true;
            }
        }
    }
    if let Some(r) = s.current_round.and_then(|i| s.rounds.get_mut(i)) {
        if r.status == "active" && r.deadline.is_some_and(|d| now >= d) {
            r.status = "finishing".into();
            r.finishing_at = r.deadline;
        }
        if r.status == "finishing" && r.finishing_at.is_some_and(|at| now >= at + 30) {
            r.status = "feedback".into();
            r.sealed_at = Some(now);
            r.final_capture_incomplete = true;
        }
    }
    if s.closed() || s.status == "pending" {
        connection.execute("DELETE FROM peer_active_users WHERE session_id=?1", [&s.id])?;
    }
    if before != serde_json::to_string(&(&*s, &s.rounds)).expect("session serializes") {
        s.version += 1;
    }
    Ok(())
}

pub(crate) fn transact<T>(
    accounts: &Accounts,
    id: &str,
    user: i64,
    now: i64,
    task: impl FnOnce(&Connection, &mut Session, usize) -> rusqlite::Result<PeerResult<T>>,
) -> rusqlite::Result<PeerResult<T>> {
    accounts.transaction(|connection| {
        let Some(mut session) = load(connection, id)? else {
            return Ok(Err("not_found"));
        };
        let Some(slot) = session.slot(user) else {
            return Ok(Err("not_found"));
        };
        let snapshot = |s: &Session| {
            serde_json::to_string(&(s, &s.rounds, &s.members)).expect("session serializes")
        };
        let before = snapshot(&session);
        settle(connection, &mut session, now)?;
        let result = task(connection, &mut session, slot)?;
        if before != snapshot(&session) {
            save(connection, &session)?;
        }
        Ok(result)
    })
}

/// Invitations one account may have waiting at once. Each stays `pending` for
/// the invite lifetime and is read by the settlement sweep every few seconds
/// under the shared database connection, so the count has to be bounded the way
/// reports and interviews are.
const MAX_PENDING_SESSIONS_PER_USER: i64 = 20;

pub(crate) fn create(
    accounts: &Accounts,
    user: i64,
    settings: Settings,
    config: &crate::config::PeerConfig,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    accounts.transaction(|connection| {
        // The cap is on sessions nobody has entered yet: an invitation,
        // accepted or not, and a pairing's session, which stays `pending` until
        // someone enters its lobby. Only a new invitation is refused by it. The
        // count reads the status column alone, since a JSON function would fail
        // for an account with one unreadable row.
        let waiting: i64 = connection.query_row(
            "SELECT count(*) FROM peer_sessions WHERE creator_id=?1 AND status='pending'",
            [user],
            |row| row.get(0),
        )?;
        if waiting >= MAX_PENDING_SESSIONS_PER_USER {
            return Ok(Err("too_many_invitations"));
        }
        create_on(connection, user, settings, config, now)
    })
}

/// Creates a pending session without the invitation cap, which `create` applies
/// to an invitation only: a pairing must not be refused for a full list.
pub(super) fn create_on(
    connection: &Connection,
    user: i64,
    settings: Settings,
    config: &crate::config::PeerConfig,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    if !valid_settings(&settings, config) {
        return Ok(Err("invalid_settings"));
    }
    let c = connection;
    let login: String = c.query_row("SELECT login FROM users WHERE id=?1", [user], |row| {
        row.get(0)
    })?;
    let s = Session {
        id: entropy()?,
        creator_id: user,
        discovery_pair: false,
        status: "pending".into(),
        version: 0,
        settings,
        created_at: now,
        expires_at: now + i64::from(config.invite_ttl_days) * 86400,
        hard_minutes: config.hard_max_minutes,
        reconnect_grace_seconds: config.reconnect_grace_seconds,
        hard_deadline: None,
        outcome: None,
        provider_id: None,
        room_name: None,
        start_confirmed: [false; 2],
        observer_consented: [false; 2],
        swap_confirmed: [false; 2],
        current_round: None,
        room_generation: 0,
        room_deleted_at: None,
        delete_attempts: 0,
        next_delete_at: 0,
        members: vec![Member {
            slot: 0,
            user_id: user,
            login,
            nickname: discovery::nickname(c, user)?,
            identity: entropy()?,
            entered_at: None,
            last_seen_at: None,
            ready: false,
        }],
        rounds: vec![],
    };
    save(c, &s)?;
    let token = invite(c, &s, now)?;
    let mut view = s.view(user, now);
    view["inviteToken"] = json!(token);
    Ok(Ok(view))
}

pub(crate) fn invite(c: &Connection, s: &Session, now: i64) -> rusqlite::Result<String> {
    c.execute("UPDATE peer_invites SET revoked_at=?2 WHERE session_id=?1 AND consumed_at IS NULL AND revoked_at IS NULL",params![s.id,now])?;
    let token = entropy()?;
    c.execute(
        "INSERT INTO peer_invites (token_hash,session_id,expires_at) VALUES (?1,?2,?3)",
        params![session_key(&token), s.id, s.expires_at],
    )?;
    Ok(token)
}

pub(crate) fn accept(
    accounts: &Accounts,
    user: i64,
    token: &str,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    accounts.transaction(|connection| accept_on(connection, user, token, now))
}

pub(super) fn accept_on(
    connection: &Connection,
    user: i64,
    token: &str,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    let id: Option<String> = connection.query_row(
            "SELECT session_id FROM peer_invites WHERE token_hash=?1 AND revoked_at IS NULL AND consumed_at IS NULL AND expires_at>?2",
            params![session_key(token), now], |row| row.get(0),
        ).optional()?;
    let Some(id) = id else {
        return Ok(Err("invite_unavailable"));
    };
    let Some(mut session) = load(connection, &id)? else {
        return Ok(Err("invite_unavailable"));
    };
    settle(connection, &mut session, now)?;
    if session.members.len() != 1 || session.creator_id == user || session.status != "pending" {
        save(connection, &session)?;
        return Ok(Err("invite_unavailable"));
    }
    let login = connection.query_row("SELECT login FROM users WHERE id=?1", [user], |row| {
        row.get(0)
    })?;
    session.members.push(Member {
        slot: 1,
        user_id: user,
        login,
        nickname: discovery::nickname(connection, user)?,
        identity: entropy()?,
        entered_at: None,
        last_seen_at: None,
        ready: false,
    });
    session.version += 1;
    connection.execute(
        "UPDATE peer_invites SET consumed_by=?2,consumed_at=?3 WHERE token_hash=?1",
        params![session_key(token), user, now],
    )?;
    save(connection, &session)?;
    Ok(Ok(session.view(user, now)))
}

pub(crate) fn list(accounts: &Accounts, user: i64, now: i64) -> rusqlite::Result<Value> {
    accounts.transaction(|connection| {
        let ids = connection.prepare("SELECT session_id FROM peer_members WHERE user_id=?1 ORDER BY rowid DESC LIMIT 100")?
            .query_map([user], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut sessions = Vec::new();
        for id in ids {
            let Some(mut session) = load(connection, &id)? else { continue };
            let version = session.version;
            settle(connection, &mut session, now)?;
            if session.version != version {
                save(connection, &session)?;
            }
            sessions.push(session.view(user, now));
        }
        Ok(json!({"sessions": sessions}))
    })
}

pub(crate) fn action(
    c: &Connection,
    s: &mut Session,
    slot: usize,
    action: &str,
    body: &Value,
    now: i64,
) -> rusqlite::Result<PeerResult<()>> {
    rounds::action(c, s, slot, action, body, now)
}

#[cfg(test)]
#[path = "../../tests/unit/accounts/peer.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests/unit/accounts/peer_settlement.rs"]
mod settlement_tests;

pub(super) fn valid_settings(settings: &Settings, config: &crate::config::PeerConfig) -> bool {
    !(settings.interviewer_first > 1
        || !["easy", "medium", "hard"].contains(&settings.difficulty.to_ascii_lowercase().as_str())
        || !["python", "javascript", "cpp", "c", "java"].contains(&settings.language.as_str())
        || settings.round_minutes == 0
        || settings.round_minutes.saturating_mul(2) > config.hard_max_minutes
        || rounds::eligible_problems(settings).len() < 2)
}

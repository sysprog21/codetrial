//! Member-only observation records are separate from AI interview reports.
use super::{Accounts, PeerResult, load};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) const MAX_TRANSCRIPT_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Segment {
    pub slot: usize,
    pub round_index: Option<usize>,
    pub started_at: i64,
    pub ended_at: i64,
    pub text: String,
}

/// Capture is capped at two hours; what it produces is kept for a day after.
const OBSERVATION_RETENTION_SECONDS: i64 = 7200 + 86400;

/// The row that carries the observation's state. A withdrawal can arrive before
/// the observer begins, so either path may be the one that creates it.
fn ensure_row(
    c: &rusqlite::Connection,
    id: &str,
    now: i64,
    expires_at: i64,
    withdrawn: bool,
) -> rusqlite::Result<()> {
    c.execute("INSERT INTO peer_observations(session_id,status,created_at,last_seen_at,expires_at,transcript_withdrawn) VALUES(?1,'observing',?2,?2,?3,?4) ON CONFLICT(session_id) DO NOTHING",params![id,now,expires_at,withdrawn]).map(|_|())
}

pub(crate) fn begin(accounts: &Accounts, id: &str, now: i64) -> rusqlite::Result<()> {
    accounts.with(|c| {
        ensure_row(
            c,
            id,
            now,
            now.saturating_add(OBSERVATION_RETENTION_SECONDS),
            false,
        )
    })
}

pub(crate) fn append(
    accounts: &Accounts,
    id: &str,
    seq: u32,
    segment: &Segment,
) -> rusqlite::Result<()> {
    accounts.transaction(|c| {
        let s=load(c,id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        if !s.settings.save_transcript { return Ok(()); }
        let withdrawn:bool=c.query_row("SELECT transcript_withdrawn FROM peer_observations WHERE session_id=?1",[id],|r|r.get(0))?;
        if withdrawn {return Ok(()); }
        let bytes:i64=c.query_row("SELECT COALESCE(sum(length(CAST(text AS BLOB))),0) FROM peer_transcripts WHERE session_id=?1",[id],|r|r.get(0))?;
        if usize::try_from(bytes).unwrap_or(usize::MAX).saturating_add(segment.text.len())>MAX_TRANSCRIPT_BYTES {
            return Err(rusqlite::Error::InvalidQuery);
        }
        c.execute("INSERT INTO peer_transcripts(session_id,seq,slot,round_idx,started_at,ended_at,text) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,seq,u8::try_from(segment.slot).map_err(|_|rusqlite::Error::InvalidQuery)?,segment.round_index.and_then(|i|u8::try_from(i).ok()),segment.started_at,segment.ended_at,segment.text])?;
        Ok(())
    })
}

pub(crate) fn status(
    accounts: &Accounts,
    id: &str,
    status: &str,
    error: Option<&str>,
) -> rusqlite::Result<()> {
    accounts.with(|c| {
        c.execute(
            "UPDATE peer_observations SET status=?2,error=?3 WHERE session_id=?1",
            params![id, status, error],
        )
        .map(|_| ())
    })
}

pub(crate) fn finish(
    accounts: &Accounts,
    id: &str,
    advice: &Value,
    error: Option<&str>,
    now: i64,
) -> rusqlite::Result<()> {
    let retained = advice
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let mut fields = serde_json::Map::new();
            for key in [
                "slot",
                "roundIndex",
                "role",
                "strengths",
                "improvements",
                "evidenceRefs",
            ] {
                if let Some(value) = item.get(key) {
                    fields.insert(key.to_owned(), value.clone());
                }
            }
            Value::Object(fields)
        })
        .collect::<Vec<_>>();
    let advice = Value::Array(retained);
    accounts.with(|c|c.execute("UPDATE peer_observations SET status=?2,advice_json=?3,error=?4,completed_at=?5,expires_at=?6 WHERE session_id=?1",params![id,if advice.as_array().is_none_or(|items|items.is_empty())&&error.is_some(){"unavailable"}else{"ready"},advice.to_string(),error,now,now.saturating_add(86400)]).map(|_|()))
}

pub(crate) fn view(
    accounts: &Accounts,
    id: &str,
    user: i64,
    transcript: bool,
) -> rusqlite::Result<PeerResult<Value>> {
    view_at(
        accounts,
        id,
        user,
        transcript,
        crate::current_epoch_seconds_i64(),
    )
}

pub(crate) fn view_at(
    accounts: &Accounts,
    id: &str,
    user: i64,
    transcript: bool,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    accounts.with(|c| {
        let Some(s)=load(c,id)? else{return Ok(Err("not_found"))};
        let Some(slot)=s.slot(user) else{return Ok(Err("not_found"))};
        if transcript && !s.settings.save_transcript {return Ok(Err("not_found"));}
        let record:Option<(String,Option<String>,String,i64,bool)>=c.query_row("SELECT status,error,advice_json,expires_at,transcript_withdrawn FROM peer_observations WHERE session_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let Some((status,error,advice,expires,withdrawn))=record else {
            if transcript{return Ok(Err("not_found"));}
            return Ok(Ok(json!({"status":"unavailable","advice":[],"error":"observer_not_started"})));
        };
        if now>=expires || (transcript&&withdrawn){return Ok(Err("not_found"));}
        if transcript {
            let rows=c.prepare("SELECT slot,round_idx,started_at,ended_at,text FROM peer_transcripts WHERE session_id=?1 ORDER BY started_at,seq")?.query_map([id],|r|Ok(Segment {slot:usize::from(r.get::<_,u8>(0)?),round_index:r.get::<_,Option<u8>>(1)?.map(usize::from),started_at:r.get(2)?,ended_at:r.get(3)?,text:r.get(4)?}))?.collect::<rusqlite::Result<Vec<_>>>()?;
            return Ok(Ok(json!({"status":status,"segments":rows,"error":error})));
        }
        let advice:Value=serde_json::from_str(&advice).map_err(|_|rusqlite::Error::InvalidQuery)?;
        let visible=if s.closed(){advice.as_array().into_iter().flatten().filter(|item|item["slot"].as_u64()==u64::try_from(slot).ok()).cloned().collect::<Vec<_>>()}else{vec![]};
        Ok(Ok(json!({"status":status,"advice":visible,"error":error})))
    })
}

pub(crate) fn withdraw(
    accounts: &Accounts,
    id: &str,
    user: i64,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    accounts.transaction(|c| {
        let Some(s)=load(c,id)? else{return Ok(Err("not_found"))};
        if s.slot(user).is_none()||!s.settings.save_transcript{return Ok(Err("not_found"));}

        // Kept until the session can no longer start, so a later start finds
        // the refusal.
        let expires_at = s.expires_at.max(now.saturating_add(OBSERVATION_RETENTION_SECONDS));
        ensure_row(c, id, now, expires_at, true)?;
        let changed=c.execute("UPDATE peer_observations SET transcript_withdrawn=1 WHERE session_id=?1 AND expires_at>?2",params![id,now])?;
        if changed==0{return Ok(Err("not_found"));}
        c.execute("DELETE FROM peer_transcripts WHERE session_id=?1",[id])?;
        Ok(Ok(json!({"withdrawn":true})))
    })
}

pub(crate) fn sweep(accounts: &Accounts, now: i64) -> rusqlite::Result<()> {
    accounts.transaction(|c| {
        c.execute("DELETE FROM peer_transcripts WHERE session_id IN (SELECT session_id FROM peer_observations WHERE expires_at<=?1)",[now])?;
        c.execute("DELETE FROM peer_observations WHERE expires_at<=?1",[now])?;
        Ok(())
    })
}

/// A withdrawal that arrived before capture has no observer to refresh it, so
/// it keeps the expiry that lets a later start find it.
pub(crate) fn recover(accounts: &Accounts, now: i64) -> rusqlite::Result<()> {
    accounts.with(|c| {
        c.execute(
            "UPDATE peer_observations SET status='unavailable',error='observer_interrupted',
             completed_at=?1,
             expires_at=CASE WHEN transcript_withdrawn=1 THEN expires_at ELSE MIN(expires_at,?2) END
             WHERE status IN ('observing','processing') AND last_seen_at<=?3 AND expires_at>?1",
            params![now, now.saturating_add(86400), now.saturating_sub(120)],
        )
        .map(|_| ())
    })
}

pub(crate) fn heartbeat(accounts: &Accounts, id: &str, now: i64) -> rusqlite::Result<()> {
    accounts.with(|c|c.execute("UPDATE peer_observations SET last_seen_at=MAX(last_seen_at,?2) WHERE session_id=?1 AND status IN ('observing','processing')",params![id,now]).map(|_|()))
}

#[cfg(test)]
#[path = "../../../tests/unit/accounts/peer/observer.rs"]
mod tests;

use super::{PeerResult, Session, entropy, round_idx};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

fn active_round(s: &Session, index: usize) -> PeerResult<&super::Round> {
    if s.current_round != Some(index) {
        return Err(
            if s.rounds
                .get(index)
                .is_some_and(|round| round.sealed_at.is_some())
            {
                "round_sealed"
            } else {
                "round_unavailable"
            },
        );
    }
    s.rounds.get(index).ok_or("round_unavailable")
}

fn active_writer(s: &Session, index: usize, slot: usize) -> PeerResult<&super::Round> {
    let round = active_round(s, index)?;
    if round.candidate_slot != slot {
        return Err("not_writer");
    }
    if !round.accepts_writes() {
        return Err("round_sealed");
    }
    Ok(round)
}

pub(crate) fn writer(
    s: &mut Session,
    slot: usize,
    index: usize,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    change_writer(s, slot, index, None, now)
}

pub(crate) fn resume_writer(
    s: &mut Session,
    slot: usize,
    index: usize,
    previous: &str,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    change_writer(s, slot, index, Some(previous), now)
}

fn change_writer(
    s: &mut Session,
    slot: usize,
    index: usize,
    previous: Option<&str>,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    let r = match active_writer(s, index, slot) {
        Ok(r) => r,
        Err(e) => return Ok(Err(e)),
    };
    if previous.is_some_and(|lease| r.writer_lease.as_deref() != Some(lease)) {
        return Ok(Err("lease_lost"));
    }
    let lease = entropy()?;
    let r = &mut s.rounds[index];
    r.writer_lease = Some(lease.clone());
    r.writer_lease_until = now + 120;
    Ok(Ok(json!({"lease":lease,"leaseUntil":r.writer_lease_until})))
}

pub(crate) fn read_snapshot(
    c: &Connection,
    s: &Session,
    index: usize,
) -> rusqlite::Result<PeerResult<Value>> {
    if s.current_round.is_none_or(|current| index > current) || s.rounds.get(index).is_none() {
        return Ok(Err("round_unavailable"));
    }
    let snapshot=c.query_row("SELECT revision,language,code,test_summary_json,updated_at FROM peer_snapshots WHERE session_id=?1 AND round_idx=?2",params![s.id,round_idx(index)?],|row|{
        let tests:String=row.get(3)?;
        Ok(json!({"revision":row.get::<_,i64>(0)?,"language":row.get::<_,String>(1)?,"code":row.get::<_,String>(2)?,"tests":serde_json::from_str::<Value>(&tests).unwrap_or(Value::Null),"updatedAt":row.get::<_,i64>(4)?}))
    }).optional()?.unwrap_or_else(||json!({"revision":0,"language":s.settings.language,"code":"","tests":null}));
    Ok(Ok(snapshot))
}

pub(crate) fn snapshot(
    c: &Connection,
    s: &mut Session,
    slot: usize,
    index: usize,
    body: &Value,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    let r = match active_writer(s, index, slot) {
        Ok(r) => r,
        Err(e) => return Ok(Err(e)),
    };
    if body["lease"].as_str() != r.writer_lease.as_deref()
        || r.writer_lease.is_none()
        || now >= r.writer_lease_until
    {
        return Ok(Err("lease_lost"));
    }
    let final_capture = body["final"].as_bool().unwrap_or(false);
    if r.status == "finishing" && !final_capture {
        return Ok(Err("final_required"));
    }
    let (Some(language), Some(code), Some(expected)) = (
        body["language"].as_str(),
        body["code"].as_str(),
        body["expectedRevision"].as_u64(),
    ) else {
        return Ok(Err("invalid_snapshot"));
    };
    if code.len() > 48 * 1024 || language != s.settings.language {
        return Ok(Err("invalid_snapshot"));
    }
    let Some(tests) = sanitize_tests(&body["tests"]) else {
        return Ok(Err("invalid_snapshot"));
    };
    let previous: i64 = c
        .query_row(
            "SELECT revision FROM peer_snapshots WHERE session_id=?1 AND round_idx=?2",
            params![s.id, round_idx(index)?],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);
    if i64::try_from(expected).ok() != Some(previous) {
        return Ok(Err("version_conflict"));
    }
    let revision = previous
        .checked_add(1)
        .ok_or(rusqlite::Error::InvalidQuery)?;
    c.execute("INSERT INTO peer_snapshots (session_id,round_idx,revision,language,code,test_summary_json,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(session_id,round_idx) DO UPDATE SET revision=excluded.revision,language=excluded.language,code=excluded.code,test_summary_json=excluded.test_summary_json,updated_at=excluded.updated_at",params![s.id,round_idx(index)?,revision,language,code,tests.to_string(),now])?;
    let r = &mut s.rounds[index];
    r.writer_lease_until = now + 120;
    if final_capture && r.status == "finishing" {
        r.status = "feedback".into();
        r.sealed_at = Some(now);
        r.final_capture_incomplete = false;
        s.version += 1;
    }
    Ok(Ok(
        json!({"revision":revision,"leaseUntil":r.writer_lease_until}),
    ))
}

const CANDIDATE_DIMENSIONS: &[&str] = &[
    "clarification",
    "reasoning",
    "correctness",
    "testing",
    "complexity",
    "technicalExplanation",
];
const INTERVIEWER_DIMENSIONS: &[&str] = &[
    "preparation",
    "questionQuality",
    "hintPacing",
    "followUp",
    "feedbackSpecificity",
];

pub(crate) fn feedback(
    c: &Connection,
    s: &Session,
    slot: usize,
    index: usize,
    body: &Value,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    let Some(r) = s.rounds.get(index) else {
        return Ok(Err("round_unavailable"));
    };
    if r.sealed_at.is_none() {
        return Ok(Err("round_unsealed"));
    }
    let candidate = r.interviewer_slot == slot;
    let dimensions = if candidate {
        CANDIDATE_DIMENSIONS
    } else {
        INTERVIEWER_DIMENSIONS
    };
    let Some(fields) = body["dimensions"].as_object() else {
        return Ok(Err("invalid_feedback"));
    };
    let submitted = body["submitted"].as_bool().unwrap_or(false);
    if fields.keys().any(|k| !dimensions.contains(&k.as_str()))
        || (submitted && fields.len() != dimensions.len())
    {
        return Ok(Err("invalid_feedback"));
    }
    for field in fields.values() {
        if !(field["score"]
            .as_u64()
            .is_some_and(|score| (1..=4).contains(&score))
            || field["score"] == "not_observed")
            || field["example"].as_str().is_none_or(|v| v.len() > 1500)
            || field["improvement"].as_str().is_none_or(|v| v.len() > 1500)
        {
            return Ok(Err("invalid_feedback"));
        }
    }
    // Only the rubric fields are kept; any other key in the request is dropped.
    let payload = json!({"submitted":submitted,"dimensions":fields.iter().map(|(name,field)|(name.clone(),json!({"score":field["score"],"example":field["example"],"improvement":field["improvement"]}))).collect::<serde_json::Map<_,_>>()});
    let already:Option<i64>=c.query_row("SELECT submitted_at FROM peer_feedback WHERE session_id=?1 AND round_idx=?2 AND author_slot=?3",params![s.id,round_idx(index)?,u8::try_from(slot).map_err(|_| rusqlite::Error::InvalidQuery)?],|row|row.get(0)).optional()?.flatten();
    if already.is_some() {
        return Ok(Err("feedback_submitted"));
    }
    c.execute("INSERT INTO peer_feedback (session_id,round_idx,author_slot,kind,rubric_version,payload_json,submitted_at) VALUES (?1,?2,?3,?4,'peer-rubric-v1',?5,?6) ON CONFLICT(session_id,round_idx,author_slot) DO UPDATE SET payload_json=excluded.payload_json,submitted_at=excluded.submitted_at",params![s.id,round_idx(index)?,u8::try_from(slot).map_err(|_| rusqlite::Error::InvalidQuery)?,if candidate{"candidate"}else{"interviewer"},payload.to_string(),submitted.then_some(now)])?;
    Ok(Ok(json!({"saved":true,"submitted":submitted})))
}

pub(crate) fn read_feedback(
    c: &Connection,
    s: &Session,
    slot: usize,
    index: usize,
    now: i64,
) -> rusqlite::Result<PeerResult<Value>> {
    let Some(r) = s.rounds.get(index) else {
        return Ok(Err("round_unavailable"));
    };
    let count:i64=c.query_row("SELECT count(*) FROM peer_feedback WHERE session_id=?1 AND round_idx=?2 AND submitted_at IS NOT NULL",params![s.id,round_idx(index)?],|row|row.get(0))?;
    let revealed = count == 2 || r.sealed_at.is_some_and(|sealed| now >= sealed + 86400);
    let rows=c.prepare("SELECT author_slot,kind,payload_json,submitted_at FROM peer_feedback WHERE session_id=?1 AND round_idx=?2 ORDER BY author_slot")?.query_map(params![s.id,round_idx(index)?],|row|Ok((usize::from(row.get::<_,u8>(0)?),row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,Option<i64>>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let feedback=rows.into_iter().filter(|(author,_,_,submitted)|*author==slot || (revealed && submitted.is_some())).map(|(author,kind,payload,submitted)|json!({"authorSlot":author,"kind":kind,"submitted":submitted.is_some(),"payload":serde_json::from_str::<Value>(&payload).unwrap_or(Value::Null)})).collect::<Vec<_>>();
    Ok(Ok(
        json!({"feedback":feedback,"revealed":revealed,"rubricVersion":"peer-rubric-v1"}),
    ))
}

/// The test summary the partner's page renders as markup, so only the fields
/// the runner produces are kept. A wrong type rejects the save; text and case
/// lists past their limits are cut, so a large run never stops the code saving.
fn sanitize_tests(value: &Value) -> Option<Value> {
    use serde_json::Map;
    fn copy_text(
        from: &Map<String, Value>,
        to: &mut Map<String, Value>,
        key: &str,
        max: usize,
    ) -> Option<()> {
        let Some(text) = from.get(key) else {
            return Some(());
        };
        let text = text.as_str()?;
        let end = text.floor_char_boundary(max);
        to.insert(key.into(), text[..end].into());
        Some(())
    }
    if value.is_null() {
        return Some(Value::Null);
    }
    let object = value.as_object()?;
    let count = |key: &str| object.get(key)?.as_u64().filter(|n| *n <= 10_000);
    let mut out = Map::new();
    out.insert("passed".into(), count("passed")?.into());
    out.insert("total".into(), count("total")?.into());
    copy_text(object, &mut out, "setupError", 8 * 1024)?;
    if let Some(flag) = object.get("runnerUnavailable") {
        out.insert("runnerUnavailable".into(), flag.as_bool()?.into());
    }
    let mut kept = Vec::new();
    for case in object.get("cases")?.as_array()?.iter().take(100) {
        let case = case.as_object()?;
        let mut item = Map::new();
        if !case.get("label")?.is_string() {
            return None;
        }
        copy_text(case, &mut item, "label", 200)?;
        let pass = case.get("pass")?;
        if !(pass.is_boolean() || pass.is_null()) {
            return None;
        }
        item.insert("pass".into(), pass.clone());
        let millis = case.get("timeMs")?.as_u64().filter(|n| *n <= 3_600_000)?;
        item.insert("timeMs".into(), millis.into());
        for key in ["got", "expected", "input", "error"] {
            copy_text(case, &mut item, key, 4 * 1024)?;
        }
        if let Some(flag) = case.get("candidate") {
            item.insert("candidate".into(), flag.as_bool()?.into());
        }
        kept.push(Value::Object(item));
    }
    out.insert("cases".into(), Value::Array(kept));
    Some(Value::Object(out))
}

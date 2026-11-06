use super::{PeerResult, Round, Session};
use rusqlite::{Connection, params};
use serde_json::Value;

fn version(s: &Session, body: &Value) -> PeerResult<()> {
    if body["expectedVersion"].as_u64() != Some(s.version) {
        Err("version_conflict")
    } else {
        Ok(())
    }
}

pub(super) fn action(
    c: &Connection,
    s: &mut Session,
    slot: usize,
    action: &str,
    body: &Value,
    now: i64,
) -> rusqlite::Result<PeerResult<()>> {
    if s.closed() {
        // Leaving a session that has already ended has nothing left to undo.
        return Ok(if action == "leave" {
            Ok(())
        } else {
            Err("session_closed")
        });
    }
    if matches!(
        action,
        "ready" | "start" | "advance" | "finish" | "terminate" | "cancel" | "invite"
    ) && let Err(e) = version(s, body)
    {
        return Ok(Err(e));
    }
    let result = match action {
        "cancel" | "terminate" => {
            s.status = if action == "cancel" {
                "cancelled"
            } else {
                "finalizing"
            }
            .into();
            s.outcome = Some("partial".into());
            for r in &mut s.rounds {
                if r.sealed_at.is_none() {
                    r.status = "feedback".into();
                    r.sealed_at = Some(now);
                    r.final_capture_incomplete = true;
                }
            }
            c.execute("DELETE FROM peer_active_users WHERE session_id=?1", [&s.id])?;
            Ok(())
        }
        "leave" => {
            s.members[slot].entered_at = None;
            s.members[slot].ready = false;
            if s.current_round.is_none() {
                s.start_confirmed = [false; 2];
            }
            c.execute(
                "DELETE FROM peer_active_users WHERE user_id=?1 AND session_id=?2",
                params![s.members[slot].user_id, s.id],
            )?;
            s.members[slot].last_seen_at = Some(now);
            Ok(())
        }
        "heartbeat" => {
            s.members[slot].last_seen_at = Some(now);
            return Ok(Ok(()));
        }
        "ready" => {
            if s.members.len() != 2
                || s.members.iter().any(|m| m.entered_at.is_none())
                || s.current_round.is_some()
            {
                Err("waiting_for_partner")
            } else {
                s.members[slot].ready = body["ready"].as_bool().unwrap_or(true);
                s.start_confirmed = [false; 2];
                Ok(())
            }
        }
        "start" => {
            if body["observerConsent"].as_bool() != Some(true) {
                return Ok(Err("observer_consent_required"));
            }
            if s.current_round.is_some() {
                Err("round_active")
            } else if s.members.len() != 2
                || s.members.iter().any(|m| !m.ready || m.entered_at.is_none())
            {
                Err("waiting_for_partner")
            } else {
                let (consented, confirmed) = (s.observer_consented[slot], s.start_confirmed[slot]);
                s.observer_consented[slot] = true;
                s.start_confirmed[slot] = true;
                if s.start_confirmed.iter().all(|x| *x) {
                    let Some(problems) = choose_problems(c, s)? else {
                        // A refused start leaves no confirmation behind.
                        s.observer_consented[slot] = consented;
                        s.start_confirmed[slot] = confirmed;
                        return Ok(Err("problems_unavailable"));
                    };
                    s.rounds = problems
                        .into_iter()
                        .enumerate()
                        .map(|(index, problem)| {
                            let interviewer = (s.settings.interviewer_first + index) % 2;
                            Round {
                                index,
                                interviewer_slot: interviewer,
                                candidate_slot: 1 - interviewer,
                                problem_id: problem,
                                status: if index == 0 { "active" } else { "prepared" }.into(),
                                version: 0,
                                started_at: (index == 0).then_some(now),
                                deadline: (index == 0)
                                    .then_some(now + i64::from(s.settings.round_minutes) * 60),
                                finishing_at: None,
                                sealed_at: None,
                                writer_lease: None,
                                writer_lease_until: 0,
                                final_capture_incomplete: false,
                            }
                        })
                        .collect();
                    s.status = "active".into();
                    s.current_round = Some(0);
                    s.hard_deadline = Some(now + i64::from(s.hard_minutes) * 60);
                    record_exposure(c, s, 0, now)?;
                }
                Ok(())
            }
        }
        "finish" => {
            let Some(index) = s.current_round else {
                return Ok(Err("round_unavailable"));
            };
            if body["roundIndex"].as_u64() != u64::try_from(index).ok() {
                Err("round_unavailable")
            } else if s.rounds[index].status != "active" {
                Err("round_sealed")
            } else {
                s.rounds[index].status = "finishing".into();
                s.rounds[index].finishing_at = Some(now);
                Ok(())
            }
        }
        "advance" => {
            let Some(index) = s.current_round else {
                return Ok(Err("round_unavailable"));
            };

            // Round 0's feedback form is replaced by the swap, so it comes
            // first.
            let feedback_given: i64 = c.query_row(
                "SELECT count(*) FROM peer_feedback WHERE session_id=?1 AND round_idx=?2 AND author_slot=?3 AND submitted_at IS NOT NULL",
                params![s.id, super::round_idx(index)?, super::round_idx(slot)?],
                |row| row.get(0),
            )?;
            if s.rounds[index].sealed_at.is_none() {
                Err("round_unsealed")
            } else if index == 0 && feedback_given == 0 {
                Err("feedback_required")
            } else {
                s.swap_confirmed[slot] = true;
                if s.swap_confirmed.iter().all(|x| *x) {
                    s.swap_confirmed = [false; 2];
                    if index == 0 {
                        s.current_round = Some(1);
                        let r = &mut s.rounds[1];
                        r.status = "active".into();
                        r.started_at = Some(now);
                        r.deadline = Some(
                            (now + i64::from(s.settings.round_minutes) * 60)
                                .min(s.hard_deadline.unwrap_or(i64::MAX)),
                        );
                        record_exposure(c, s, 1, now)?;
                    } else {
                        s.status = "complete".into();
                        s.outcome = Some("complete".into());
                        c.execute("DELETE FROM peer_active_users WHERE session_id=?1", [&s.id])?;
                    }
                }
                Ok(())
            }
        }
        _ => Err("invalid_action"),
    };
    if result.is_ok() {
        s.version += 1;
    }
    Ok(result)
}

fn choose_problems(c: &Connection, s: &Session) -> rusqlite::Result<Option<Vec<String>>> {
    let seen = c
        .prepare("SELECT DISTINCT problem_id FROM peer_problem_exposures WHERE user_id IN (?1,?2)")?
        .query_map(params![s.members[0].user_id, s.members[1].user_id], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
    let mut candidates = eligible_problems(&s.settings)
        .into_iter()
        .map(|p| (seen.contains(p.id), p.id.to_owned()))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(seen, _)| *seen);
    if candidates.len() < 2 {
        return Ok(None);
    }
    Ok(Some(
        candidates.into_iter().take(2).map(|(_, id)| id).collect(),
    ))
}

fn record_exposure(c: &Connection, s: &Session, index: usize, now: i64) -> rusqlite::Result<()> {
    let r = &s.rounds[index];
    for m in &s.members {
        c.execute("INSERT OR IGNORE INTO peer_problem_exposures (user_id,problem_id,kind,at) VALUES (?1,?2,?3,?4)",params![m.user_id,r.problem_id,if m.slot==r.interviewer_slot{"interviewer"}else{"candidate"},now])?;
    }
    Ok(())
}

pub(super) fn eligible_problems(settings: &super::Settings) -> Vec<&'static crate::agent::Problem> {
    crate::agent::PROBLEMS
        .iter()
        .filter(|problem| {
            problem
                .difficulty
                .eq_ignore_ascii_case(&settings.difficulty)
                && crate::agent::variant_for(problem.id).is_some()
                && (settings.topic.is_empty()
                    || crate::agent::topics_for(problem.id).is_some_and(|topics| {
                        topics
                            .iter()
                            .any(|topic| topic.eq_ignore_ascii_case(&settings.topic))
                    }))
        })
        .collect()
}

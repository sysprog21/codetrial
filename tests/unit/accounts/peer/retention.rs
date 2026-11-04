use super::*;
use crate::accounts::peer::{Settings, create};

const DAY: i64 = 86_400;

fn fixture() -> Accounts {
    let a = Accounts::open(crate::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "retention-test".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    a.initialize_schema().unwrap();
    a.with(|c| c.execute_batch("INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES(1,1,'alice',1,1),(2,2,'bob',1,1)")).unwrap();
    a
}

fn session(a: &Accounts, user: i64, created: i64, status: &str) -> String {
    let made = create(
        a,
        user,
        Settings {
            topic: String::new(),
            difficulty: "easy".into(),
            language: "python".into(),
            round_minutes: 25,
            interviewer_first: 0,
            save_transcript: false,
            save_audio: false,
        },
        &crate::config::PeerConfig::default(),
        created,
    )
    .unwrap()
    .unwrap();
    let id = made["sessionId"].as_str().unwrap().to_owned();
    a.with(|c| {
        c.execute(
            "UPDATE peer_sessions SET status=?2,created_at=?3 WHERE id=?1",
            rusqlite::params![id, status, created],
        )?;
        // Closing releases the member's reservation, as settlement does.
        c.execute("DELETE FROM peer_active_users WHERE session_id=?1", [&id])
    })
    .unwrap();
    id
}

fn exists(a: &Accounts, table: &str, id: &str) -> bool {
    a.with(|c| {
        c.query_row(
            &format!("SELECT count(*) FROM {table} WHERE session_id=?1"),
            [id],
            |r| r.get::<_, i64>(0),
        )
    })
    .unwrap()
        > 0
}

#[test]
fn a_closed_session_is_pruned_with_everything_it_owns_after_the_retention_period() {
    let a = fixture();
    let now = 100 * DAY;
    let old = session(&a, 1, now - RETENTION_SECONDS - 1, "complete");
    a.with(|c| {
        c.execute("INSERT INTO peer_observations(session_id,status,created_at,last_seen_at,expires_at) VALUES(?1,'ready',1,1,2)", [&old])?;
        c.execute("INSERT INTO peer_transcripts(session_id,seq,slot,started_at,ended_at,text) VALUES(?1,0,0,1,2,'x')", [&old])?;
        c.execute("INSERT INTO peer_audio_records(session_id,status,created_at,expires_at,purged_at) VALUES(?1,'deleted',1,2,3)", [&old])
    })
    .unwrap();

    assert_eq!(prune(&a, now).unwrap(), 1);
    for table in [
        "peer_members",
        "peer_observations",
        "peer_transcripts",
        "peer_audio_records",
    ] {
        assert!(!exists(&a, table, &old), "{table} outlived its session");
    }
    let sessions: i64 = a
        .with(|c| c.query_row("SELECT count(*) FROM peer_sessions", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(sessions, 0);
}

#[test]
fn rounds_snapshots_feedback_and_invites_go_with_their_session() {
    let a = fixture();
    let now = 100 * DAY;
    let old = session(&a, 1, now - RETENTION_SECONDS - 1, "complete");
    a.with(|c| {
        c.execute("INSERT INTO peer_rounds(session_id,idx,interviewer_slot,candidate_slot,problem_id,status,version,state_json) VALUES(?1,0,0,1,'p','feedback',1,'{}')", [&old])?;
        c.execute("INSERT INTO peer_snapshots(session_id,round_idx,revision,language,code,test_summary_json,updated_at) VALUES(?1,0,1,'python','x','{}',1)", [&old])?;
        c.execute("INSERT INTO peer_feedback(session_id,round_idx,author_slot,kind,rubric_version,payload_json) VALUES(?1,0,0,'human','v1','{}')", [&old])?;
        c.execute("INSERT INTO peer_invites(token_hash,session_id,expires_at) VALUES('h',?1,1)", [&old])
    })
    .unwrap();

    assert_eq!(prune(&a, now).unwrap(), 1);
    for table in [
        "peer_rounds",
        "peer_snapshots",
        "peer_feedback",
        "peer_invites",
    ] {
        assert!(!exists(&a, table, &old), "{table} outlived its session");
    }
}

#[test]
fn a_terminated_session_and_its_paired_lobby_entry_are_pruned() {
    let a = fixture();
    let now = 100 * DAY;
    let old = session(&a, 1, now - RETENTION_SECONDS - 1, "finalizing");
    a.with(|c| {
        c.execute(
            "INSERT INTO peer_availability(availability_id,user_id,status,settings_json,provider_id,room_name,participant_identity,created_at,expires_at,session_id) VALUES('av',1,'paired','{}','p','room-av','id',1,2,?1)",
            [&old],
        )
    })
    .unwrap();

    assert_eq!(prune(&a, now).unwrap(), 1);
    let left: i64 = a
        .with(|c| c.query_row("SELECT count(*) FROM peer_availability", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(left, 0);
}

#[test]
fn a_session_inside_the_period_or_still_open_is_kept() {
    let a = fixture();
    let now = 100 * DAY;
    let recent = session(&a, 1, now - RETENTION_SECONDS + 10, "complete");
    let open = session(&a, 2, now - 90 * DAY, "active");

    assert_eq!(prune(&a, now).unwrap(), 0);
    assert!(exists(&a, "peer_members", &recent));
    assert!(exists(&a, "peer_members", &open));
}

#[test]
fn audio_whose_file_may_remain_keeps_its_session() {
    let a = fixture();
    let now = 100 * DAY;
    let id = session(&a, 1, now - RETENTION_SECONDS - 1, "complete");
    a.with(|c| c.execute("INSERT INTO peer_audio_records(session_id,status,created_at,expires_at) VALUES(?1,'deleted',1,2)", [&id]))
        .unwrap();

    assert_eq!(
        prune(&a, now).unwrap(),
        0,
        "a failed unlink is retried first"
    );
    a.with(|c| {
        c.execute(
            "UPDATE peer_audio_records SET purged_at=3 WHERE session_id=?1",
            [&id],
        )
    })
    .unwrap();
    assert_eq!(prune(&a, now).unwrap(), 1);
}

#[test]
fn a_room_not_yet_deleted_keeps_its_session() {
    let a = fixture();
    let now = 100 * DAY;
    let id = session(&a, 1, now - RETENTION_SECONDS - 1, "cancelled");
    a.with(|c| {
        c.execute(
            "UPDATE peer_sessions SET room_name='room-x',room_deleted_at=NULL WHERE id=?1",
            [&id],
        )
    })
    .unwrap();

    assert_eq!(
        prune(&a, now).unwrap(),
        0,
        "the room cleanup still needs the row"
    );
    a.with(|c| {
        c.execute(
            "UPDATE peer_sessions SET room_deleted_at=5 WHERE id=?1",
            [&id],
        )
    })
    .unwrap();
    assert_eq!(prune(&a, now).unwrap(), 1);
}

#[test]
fn finished_requests_and_availability_go_but_a_live_one_stays() {
    let a = fixture();
    let now = 100 * DAY;
    let old = now - RETENTION_SECONDS - 1;
    a.with(|c| {
        for (id, user, status, created, deleted) in [
            ("old-expired", 1, "expired", old, Some(5)),
            ("old-unreleased", 2, "expired", old, None),
            ("live", 2, "available", old, None),
        ] {
            c.execute(
                "INSERT INTO peer_availability(availability_id,user_id,status,settings_json,provider_id,room_name,participant_identity,created_at,expires_at,room_deleted_at) VALUES(?1,?2,?3,'{}','p',?4,'i',?5,?5,?6)",
                rusqlite::params![id, user, status, format!("room-{id}"), created, deleted],
            )?;
        }
        c.execute("INSERT INTO peer_pair_requests(request_id,availability_id,from_user_id,to_user_id,status,settings_json,created_at,expires_at) VALUES('r-old','old-expired',2,1,'declined','{}',?1,?1)", [old])?;
        c.execute("INSERT INTO peer_pair_requests(request_id,availability_id,from_user_id,to_user_id,status,settings_json,created_at,expires_at) VALUES('r-live','live',1,2,'pending','{}',?1,?2)", [old, now + DAY])
    })
    .unwrap();

    prune(&a, now).unwrap();
    let left = |sql: &str| -> Vec<String> {
        a.with(|c| {
            c.prepare(sql)?
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<Vec<String>>>()
        })
        .unwrap()
    };
    assert_eq!(
        left("SELECT availability_id FROM peer_availability ORDER BY 1"),
        ["live", "old-unreleased"]
    );
    assert_eq!(
        left("SELECT request_id FROM peer_pair_requests"),
        ["r-live"]
    );
}

#[test]
fn closed_sessions_are_kept_for_exactly_thirty_days() {
    assert_eq!(RETENTION_SECONDS, 30 * DAY);
    let a = fixture();
    let now = 100 * DAY;
    let kept = session(&a, 1, now - 30 * DAY, "complete");
    let gone = session(&a, 2, now - 30 * DAY - 1, "complete");

    assert_eq!(prune(&a, now).unwrap(), 1);
    assert!(exists(&a, "peer_members", &kept));
    assert!(!exists(&a, "peer_members", &gone));
}

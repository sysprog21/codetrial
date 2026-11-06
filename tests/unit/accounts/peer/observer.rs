use super::*;
use crate::accounts::peer::{Settings, create};

fn fixture() -> (Accounts, String) {
    let a = Accounts::open(crate::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "observer-test".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    a.initialize_schema().unwrap();
    a.with(|c| {
        c.execute_batch(
            "INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES(1,1,'alice',1,1)",
        )
    })
    .unwrap();
    let made = create(
        &a,
        1,
        Settings {
            topic: String::new(),
            difficulty: "easy".into(),
            language: "python".into(),
            round_minutes: 25,
            interviewer_first: 0,
            save_transcript: true,
            save_audio: false,
        },
        &crate::config::PeerConfig::default(),
        100,
    )
    .unwrap()
    .unwrap();
    let id = made["sessionId"].as_str().unwrap().to_owned();
    (a, id)
}

fn segment(text: String) -> Segment {
    Segment {
        slot: 0,
        round_index: Some(0),
        started_at: 1,
        ended_at: 2,
        text,
    }
}

#[test]
fn an_observation_is_kept_for_the_capture_window_plus_a_day() {
    let (a, id) = fixture();
    begin(&a, &id, 1_000).unwrap();
    let expires: i64 = a
        .with(|c| {
            c.query_row(
                "SELECT expires_at FROM peer_observations WHERE session_id=?1",
                [&id],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(expires, 1_000 + 2 * 3_600 + 86_400);
}

#[test]
fn the_transcript_is_capped_at_half_a_mebibyte_in_total() {
    let (a, id) = fixture();
    begin(&a, &id, 1_000).unwrap();
    append(&a, &id, 0, &segment("a".repeat(MAX_TRANSCRIPT_BYTES - 1))).unwrap();
    append(&a, &id, 1, &segment("b".into())).unwrap();
    assert!(
        append(&a, &id, 2, &segment("c".into())).is_err(),
        "one byte past the cap is refused"
    );
    let stored: i64 = a
        .with(|c| {
            c.query_row(
                "SELECT sum(length(text)) FROM peer_transcripts WHERE session_id=?1",
                [&id],
                |r| r.get(0),
            )
        })
        .unwrap();
    assert_eq!(stored, 512 * 1024);
}

#[test]
fn a_status_change_is_recorded_with_its_error() {
    let (a, id) = fixture();
    begin(&a, &id, 1_000).unwrap();
    status(&a, &id, "processing", Some("slow")).unwrap();
    let viewed = view_at(&a, &id, 1, false, 1_001).unwrap().unwrap();
    assert_eq!(viewed["status"], "processing");
    assert_eq!(viewed["error"], "slow");
}

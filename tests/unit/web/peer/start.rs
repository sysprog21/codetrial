use super::*;
use serde_json::json;

fn session() -> peer::Session {
    let accounts = Accounts::open(crate::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "prestart-test".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    accounts.initialize_schema().unwrap();
    accounts.with(|c|c.execute_batch("INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES(1,1,'alice',1,1),(2,2,'bob',1,1)")).unwrap();
    let created = peer::create(
        &accounts,
        1,
        peer::Settings {
            topic: String::new(),
            difficulty: "easy".into(),
            language: "python".into(),
            round_minutes: 25,
            interviewer_first: 0,
            save_transcript: false,
            save_audio: false,
        },
        &crate::config::PeerConfig::default(),
        100,
    )
    .unwrap()
    .unwrap();
    peer::accept(&accounts, 2, created["inviteToken"].as_str().unwrap(), 101)
        .unwrap()
        .unwrap();
    let mut session = accounts
        .with(|c| peer::load(c, created["sessionId"].as_str().unwrap()))
        .unwrap()
        .unwrap();
    for member in &mut session.members {
        member.entered_at = Some(102);
        member.ready = true;
    }
    session.provider_id = Some("primary".into());
    session
}

#[test]
fn preparing_both_consents_does_not_start_the_clock_before_the_observer_joins() {
    let mut s = session();
    let version = s.version;
    let first = prepare(
        &mut s,
        0,
        &json!({"expectedVersion":version,"observerConsent":true}),
    )
    .unwrap();
    assert!(!first.consented);
    assert_eq!(first.version, version + 1);
    let second = prepare(
        &mut s,
        1,
        &json!({"expectedVersion":first.version,"observerConsent":true}),
    )
    .unwrap();
    assert!(second.consented);
    assert_eq!(second.version, version + 2);
    assert_eq!(second.provider.as_deref(), Some("primary"));
    assert!(s.current_round.is_none());
    assert!(s.hard_deadline.is_none());
    assert!(s.rounds.is_empty());
    assert_eq!(s.observer_consented, [true, true]);
}

#[test]
fn prestart_refuses_stale_or_absent_consent_and_unready_members_without_granting_consent() {
    let mut s = session();
    let version = s.version;
    assert_eq!(
        prepare(&mut s, 0, &json!({"expectedVersion":version})).err(),
        Some("observer_consent_required")
    );
    assert_eq!(
        prepare(
            &mut s,
            0,
            &json!({"expectedVersion":version+1,"observerConsent":true})
        )
        .err(),
        Some("version_conflict")
    );
    s.members[1].ready = false;
    assert_eq!(
        prepare(
            &mut s,
            0,
            &json!({"expectedVersion":version,"observerConsent":true})
        )
        .err(),
        Some("waiting_for_partner")
    );
    assert_eq!(s.observer_consented, [false, false]);
    assert_eq!(s.version, version);
}

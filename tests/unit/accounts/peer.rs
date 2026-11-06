use super::*;

fn accounts() -> Accounts {
    let a = Accounts::open(crate::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "peer-test".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    a.initialize_schema().unwrap();
    a.with(|c|c.execute_batch("INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES(1,1,'alice',1,1),(2,2,'bob',1,1),(3,3,'eve',1,1)")).unwrap();
    a
}

#[test]
fn invitation_membership_is_fixed_and_accepting_reserves_nothing() {
    let a = accounts();
    let config = crate::config::PeerConfig::default();
    let created = create(
        &a,
        1,
        Settings {
            topic: String::new(),
            difficulty: "easy".into(),
            language: "python".into(),
            round_minutes: 25,
            interviewer_first: 0,
            save_transcript: false,
            save_audio: false,
        },
        &config,
        100,
    )
    .unwrap()
    .unwrap();
    let token = created["inviteToken"].as_str().unwrap();
    let id = created["sessionId"].as_str().unwrap();
    assert_eq!(
        accept(&a, 1, token, 101).unwrap().unwrap_err(),
        "invite_unavailable"
    );
    let state = accept(&a, 2, token, 102).unwrap().unwrap();
    assert_eq!(state["members"].as_array().unwrap().len(), 2);
    assert_eq!(
        accept(&a, 3, token, 103).unwrap().unwrap_err(),
        "invite_unavailable"
    );
    a.with(|c| {
        let count: i64 = c.query_row("SELECT count(*) FROM peer_active_users", [], |r| r.get(0))?;
        assert_eq!(count, 0);
        let s = load(c, id)?.unwrap();
        assert!(s.room_name.is_none());
        assert!(s.hard_deadline.is_none());
        Ok(())
    })
    .unwrap();
}

#[test]
fn an_unavailable_topic_is_refused_before_an_invite_is_created() {
    let a = accounts();
    let settings = Settings {
        topic: "missing-topic".into(),
        difficulty: "easy".into(),
        language: "python".into(),
        round_minutes: 25,
        interviewer_first: 0,
        save_transcript: false,
        save_audio: false,
    };
    assert_eq!(
        create(&a, 1, settings, &crate::config::PeerConfig::default(), 100)
            .unwrap()
            .unwrap_err(),
        "invalid_settings"
    );
    a.with(|c| {
        let count: i64 = c.query_row("SELECT count(*) FROM peer_sessions", [], |row| row.get(0))?;
        assert_eq!(count, 0);
        Ok(())
    })
    .unwrap();
}

#[test]
fn both_confirm_current_state_and_new_sessions_avoid_exposed_problems() {
    let a = accounts();
    let mut seen = Vec::new();
    for now in [100, 1000] {
        let created = create(
            &a,
            1,
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
            now,
        )
        .unwrap()
        .unwrap();
        accept(&a, 2, created["inviteToken"].as_str().unwrap(), now + 1)
            .unwrap()
            .unwrap();
        let id = created["sessionId"].as_str().unwrap();
        a.with(|c| {
            let mut s = load(c, id)?.unwrap();
            s.status = "lobby".into();
            for m in &mut s.members {
                m.ready = true;
                m.entered_at = Some(now + 2);
            }
            let version = s.version;
            action(
                c,
                &mut s,
                0,
                "start",
                &json!({"expectedVersion":version,"observerConsent":true}),
                now + 3,
            )?
            .unwrap();
            assert!(
                s.current_round.is_none(),
                "one confirmation cannot start the clock"
            );
            assert_eq!(
                action(
                    c,
                    &mut s,
                    1,
                    "start",
                    &json!({"expectedVersion":version,"observerConsent":true}),
                    now + 4
                )?
                .unwrap_err(),
                "version_conflict"
            );
            let version = s.version;
            action(
                c,
                &mut s,
                1,
                "start",
                &json!({"expectedVersion":version,"observerConsent":true}),
                now + 4,
            )?
            .unwrap();
            assert_eq!(s.current_round, Some(0));
            assert_eq!(s.rounds.len(), 2);
            assert_ne!(s.rounds[0].problem_id, s.rounds[1].problem_id);
            assert_ne!(s.rounds[0].candidate_slot, s.rounds[1].candidate_slot);
            assert_eq!(s.rounds[0].deadline, Some(now + 4 + 25 * 60));
            assert_eq!(s.hard_deadline, Some(now + 4 + 90 * 60));
            assert!(!seen.contains(&s.rounds[0].problem_id));
            seen.push(s.rounds[0].problem_id.clone());
            save(c, &s)
        })
        .unwrap();
    }
}

#[test]
fn creation_refuses_languages_without_a_browser_test_runner() {
    let a = accounts();
    for language in ["rust", "go", "typescript"] {
        let result = create(
            &a,
            1,
            Settings {
                topic: String::new(),
                difficulty: "easy".into(),
                language: language.into(),
                round_minutes: 25,
                interviewer_first: 0,
                save_transcript: false,
                save_audio: false,
            },
            &crate::config::PeerConfig::default(),
            100,
        )
        .unwrap();
        assert_eq!(result.unwrap_err(), "invalid_settings");
    }
}

fn easy() -> Settings {
    Settings {
        topic: String::new(),
        difficulty: "easy".into(),
        language: "python".into(),
        round_minutes: 25,
        interviewer_first: 0,
        save_transcript: false,
        save_audio: false,
    }
}

#[test]
fn a_new_session_stores_its_settings_and_expires_after_the_invite_lifetime() {
    let a = accounts();
    let config = crate::config::PeerConfig {
        invite_ttl_days: 3,
        ..Default::default()
    };
    let mut settings = easy();
    settings.topic = "Array".into();
    settings.save_audio = true;
    let created = create(&a, 1, settings.clone(), &config, 100)
        .unwrap()
        .unwrap();
    let id = created["sessionId"].as_str().unwrap();
    a.with(|c| {
        let session = load(c, id)?.unwrap();
        assert_eq!(session.expires_at, 100 + 3 * 86_400);
        let stored: String = c.query_row(
            "SELECT settings_json FROM peer_sessions WHERE id=?1",
            [id],
            |row| row.get(0),
        )?;
        assert_eq!(
            serde_json::from_str::<Value>(&stored).unwrap(),
            serde_json::to_value(&settings).unwrap()
        );
        let invite_expiry: i64 = c.query_row(
            "SELECT expires_at FROM peer_invites WHERE session_id=?1",
            [id],
            |row| row.get(0),
        )?;
        assert_eq!(invite_expiry, 100 + 3 * 86_400);
        Ok(())
    })
    .unwrap();
}

#[test]
fn accepting_an_invite_advances_the_session_version_by_one() {
    let a = accounts();
    let created = create(&a, 1, easy(), &crate::config::PeerConfig::default(), 100)
        .unwrap()
        .unwrap();
    assert_eq!(created["version"], 0);
    let token = created["inviteToken"].as_str().unwrap();
    let joined = accept(&a, 2, token, 101).unwrap().unwrap();
    assert_eq!(joined["version"], 1);
    a.with(|c| {
        let id = created["sessionId"].as_str().unwrap();
        assert_eq!(load(c, id)?.unwrap().version, 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn listing_returns_the_members_sessions_and_persists_their_settlement() {
    let a = accounts();
    let config = crate::config::PeerConfig::default();
    let created = create(&a, 1, easy(), &config, 100).unwrap().unwrap();
    let id = created["sessionId"].as_str().unwrap();
    let listed = list(&a, 1, 101).unwrap();
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["sessionId"], id);
    assert_eq!(sessions[0]["status"], "pending");
    assert!(
        list(&a, 2, 101).unwrap()["sessions"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let after_expiry = 100 + i64::from(config.invite_ttl_days) * 86_400;
    let listed = list(&a, 1, after_expiry).unwrap();
    assert_eq!(listed["sessions"][0]["status"], "cancelled");
    a.with(|c| {
        let stored = load(c, id)?.unwrap();
        assert_eq!(
            stored.status, "cancelled",
            "settlement is saved, not only shown"
        );
        assert_eq!(stored.version, 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn each_invalid_setting_is_refused_on_its_own() {
    let config = crate::config::PeerConfig::default();
    assert!(valid_settings(&easy(), &config));
    let mut upper = easy();
    upper.difficulty = "EASY".into();
    assert!(valid_settings(&upper, &config), "difficulty ignores case");
    let mut second_slot = easy();
    second_slot.interviewer_first = 1;
    assert!(valid_settings(&second_slot, &config));
    let mut two_problems = easy();
    two_problems.topic = "Binary Search".into();
    assert!(
        valid_settings(&two_problems, &config),
        "exactly two problems are enough for two rounds"
    );
    let mut longest_round = easy();
    longest_round.round_minutes = config.hard_max_minutes / 2;
    assert!(
        valid_settings(&longest_round, &config),
        "two rounds fit exactly"
    );

    type Change = fn(&mut Settings);
    let invalid: [(&str, Change); 6] = [
        ("third interviewer slot", |s| s.interviewer_first = 2),
        ("unknown difficulty", |s| s.difficulty = "extreme".into()),
        ("language without a runner", |s| s.language = "rust".into()),
        ("zero minutes", |s| s.round_minutes = 0),
        ("rounds past the hard limit", |s| {
            s.round_minutes = 90 / 2 + 1;
        }),
        ("topic without two problems", |s| {
            s.topic = "Memoization".into();
        }),
    ];
    for (name, change) in invalid {
        let mut settings = easy();
        change(&mut settings);
        assert!(!valid_settings(&settings, &config), "{name}");
    }
}

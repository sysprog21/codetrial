use super::*;

fn fixture() -> Accounts {
    let a = Accounts::open(crate::accounts::GitHubLoginConfig {
        oauth: None,
        session_secret: "discovery-test".into(),
        db_path: ":memory:".into(),
        oauth_base_url: String::new(),
        api_base_url: String::new(),
    })
    .unwrap();
    a.initialize_schema().unwrap();
    a.with(|c|c.execute_batch("INSERT INTO users(id,github_id,login,created_at,updated_at)VALUES(1,1,'alice',1,1),(2,2,'bob',1,1),(3,3,'eve',1,1)")).unwrap();
    a
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
fn available(a: &Accounts, user: i64, now: i64) -> Availability {
    set_profile(a, user, if user == 1 { "Alice" } else { "Bob" }, now)
        .unwrap()
        .unwrap();
    let row = prepare(
        a,
        user,
        easy(),
        Some((&format!("interview-wait{user}"), "primary")),
        &crate::config::PeerConfig::default(),
        now,
    )
    .unwrap()
    .unwrap();
    publish(a, &row.id, user, true, now).unwrap().unwrap()
}

#[test]
fn refreshing_an_available_entry_needs_no_provider_but_a_new_room_does() {
    let a = fixture();
    let row = available(&a, 1, 100);
    let config = crate::config::PeerConfig::default();
    let refreshed = prepare(&a, 1, easy(), None, &config, 101).unwrap().unwrap();
    assert_eq!(refreshed.id, row.id);
    set_profile(&a, 2, "Bob", 100).unwrap().unwrap();
    assert!(matches!(
        prepare(&a, 2, easy(), None, &config, 101).unwrap(),
        Err("provider_unavailable")
    ));
}

#[test]
fn nickname_is_required_and_rejects_numeric_non_ascii_and_punctuation() {
    let a = fixture();
    assert!(profile(&a, 1).unwrap()["nickname"].is_null());
    assert!(matches!(
        prepare(
            &a,
            1,
            easy(),
            Some(("interview-wait1", "primary")),
            &crate::config::PeerConfig::default(),
            100
        )
        .unwrap(),
        Err("nickname_required")
    ));
    for name in [
        "",
        "1234",
        "Alice Bob",
        "Alice_1",
        "éclair",
        "longnickname123456789012345",
    ] {
        assert_eq!(
            set_profile(&a, 1, name, 100).unwrap().unwrap_err(),
            "invalid_nickname"
        );
    }
    assert_eq!(
        set_profile(&a, 1, "  Alice7  ", 100).unwrap().unwrap()["nickname"],
        "Alice7"
    );
}

#[test]
fn only_published_fresh_availability_is_in_the_roster() {
    let a = fixture();
    set_profile(&a, 1, "Alice", 100).unwrap().unwrap();
    let pending = prepare(
        &a,
        1,
        easy(),
        Some(("interview-wait1", "primary")),
        &crate::config::PeerConfig::default(),
        100,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        lobby(&a, 2, 100).unwrap()["available"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    publish(&a, &pending.id, 1, true, 101).unwrap().unwrap();
    let roster = lobby(&a, 2, 102).unwrap();
    let people = roster["available"].as_array().unwrap();
    assert_eq!(people.len(), 1);
    assert_eq!(people[0]["nickname"], "Alice");
    assert_eq!(people[0]["login"], "alice");
    assert!(people[0].get("email").is_none());
    assert!(people[0].get("identity").is_none());
    assert!(
        lobby(&a, 3, 145).unwrap()["available"]
            .as_array()
            .unwrap()
            .len()
            == 1
    );
    assert_eq!(
        lobby(&a, 3, 146).unwrap()["available"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        heartbeat(&a, 1, 146).unwrap().unwrap_err(),
        "availability_unavailable"
    );
}

#[test]
fn both_members_consent_to_the_same_proposal_before_any_session_is_created() {
    let a = fixture();
    let host = available(&a, 1, 100);
    set_profile(&a, 2, "Bob", 100).unwrap().unwrap();
    let request = request(&a, 2, &host.id, 101).unwrap().unwrap();
    let id = request["requestId"].as_str().unwrap();
    let config = crate::config::PeerConfig::default();
    assert_eq!(
        confirm(&a, 2, id, false, &config, 102)
            .unwrap()
            .unwrap_err(),
        "observer_consent_required"
    );
    assert_eq!(
        confirm(&a, 3, id, true, &config, 102).unwrap().unwrap_err(),
        "not_found"
    );
    let first = confirm(&a, 2, id, true, &config, 102).unwrap().unwrap();
    assert_eq!(first["status"], "pending");
    assert!(first["sessionId"].is_null());
    a.with(|c| {
        let count: i64 = c.query_row("SELECT count(*) FROM peer_sessions", [], |r| r.get(0))?;
        assert_eq!(count, 0);
        Ok(())
    })
    .unwrap();
    let paired = confirm(&a, 1, id, true, &config, 103).unwrap().unwrap();
    assert_eq!(paired["status"], "paired");
    assert!(paired.get("inviteToken").is_none());
    let session_id = paired["sessionId"].as_str().unwrap();
    a.with(|c| {
        let s = load(c, session_id)?.unwrap();
        assert_eq!(s.members.len(), 2);
        assert_eq!(s.members[0].identity, host.identity);
        assert_eq!(s.provider_id.as_deref(), Some("primary"));
        assert_eq!(s.room_name.as_deref(), Some(host.room.as_str()));
        assert!(s.current_round.is_none());
        assert!(s.hard_deadline.is_none());
        assert_eq!(s.observer_consented, [true, true]);
        assert!(s.members.iter().all(|m| m.entered_at.is_none()));
        Ok(())
    })
    .unwrap();
    assert_eq!(
        lobby(&a, 3, 104).unwrap()["available"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        confirm(&a, 1, id, true, &config, 104).unwrap().unwrap()["sessionId"],
        session_id
    );
}

#[test]
fn opt_out_expires_requests_and_room_cleanup_is_claimed_once() {
    let a = fixture();
    let host = available(&a, 1, 100);
    set_profile(&a, 2, "Bob", 100).unwrap().unwrap();
    let r = request(&a, 2, &host.id, 101).unwrap().unwrap();
    leave(&a, 1, 102).unwrap();
    assert_eq!(
        confirm(
            &a,
            2,
            r["requestId"].as_str().unwrap(),
            true,
            &crate::config::PeerConfig::default(),
            102
        )
        .unwrap()
        .unwrap_err(),
        "request_unavailable"
    );
    let rooms = cleanup_candidates(&a, 102).unwrap();
    assert_eq!(rooms.len(), 1);
    assert!(cleanup_candidates(&a, 103).unwrap().is_empty());
    cleanup_finished(&a, &host.id, true, 103).unwrap();
    assert!(cleanup_candidates(&a, 1000).unwrap().is_empty());
}

#[test]
fn abandoned_confirmed_pair_releases_its_bound_room_after_ten_minutes() {
    let a = fixture();
    let host = available(&a, 1, 100);
    set_profile(&a, 2, "Bob", 100).unwrap().unwrap();
    let r = request(&a, 2, &host.id, 101).unwrap().unwrap();
    let id = r["requestId"].as_str().unwrap();
    let config = crate::config::PeerConfig::default();
    confirm(&a, 1, id, true, &config, 102).unwrap().unwrap();
    let paired = confirm(&a, 2, id, true, &config, 103).unwrap().unwrap();
    let session_id = paired["sessionId"].as_str().unwrap();
    a.with(|c| {
        assert_eq!(
            super::super::room_reservations(c, Some(session_id))?,
            (1, true)
        );
        let mut s = load(c, session_id)?.unwrap();
        super::super::settle(c, &mut s, 702)?;
        assert_eq!(s.status, "pending");
        super::super::settle(c, &mut s, 703)?;
        assert_eq!(s.status, "cancelled");
        assert_eq!(s.outcome.as_deref(), Some("no_show"));
        save(c, &s)?;
        assert_eq!(
            super::super::room_reservations(c, Some(session_id))?,
            (0, false)
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn persistence_choices_are_independent_and_immutable_in_a_pairing_proposal() {
    for (transcript, audio) in [(false, false), (false, true), (true, false), (true, true)] {
        let a = fixture();
        set_profile(&a, 1, "Alice", 100).unwrap().unwrap();
        set_profile(&a, 2, "Bob", 100).unwrap().unwrap();
        let mut choices = easy();
        choices.save_transcript = transcript;
        choices.save_audio = audio;
        let host = prepare(
            &a,
            1,
            choices.clone(),
            Some(("interview-wait1", "primary")),
            &crate::config::PeerConfig::default(),
            100,
        )
        .unwrap()
        .unwrap();
        publish(&a, &host.id, 1, true, 100).unwrap().unwrap();
        let mut changed = choices.clone();
        changed.save_audio = !audio;
        assert!(matches!(
            prepare(
                &a,
                1,
                changed,
                Some(("interview-unused", "primary")),
                &crate::config::PeerConfig::default(),
                101
            )
            .unwrap(),
            Err("settings_immutable")
        ));
        let r = request(&a, 2, &host.id, 101).unwrap().unwrap();
        assert_eq!(r["settings"]["saveTranscript"], transcript);
        assert_eq!(r["settings"]["saveAudio"], audio);
        let id = r["requestId"].as_str().unwrap();
        let config = crate::config::PeerConfig::default();
        confirm(&a, 1, id, true, &config, 102).unwrap().unwrap();
        let paired = confirm(&a, 2, id, true, &config, 103).unwrap().unwrap();
        a.with(|c| {
            let s = load(c, paired["sessionId"].as_str().unwrap())?.unwrap();
            assert_eq!(s.settings, choices);
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn a_member_entering_a_private_session_disappears_from_discovery() {
    let a = fixture();
    let host = available(&a, 1, 100);
    let created = create_on_fixture(&a, 101);
    a.with(|c| {
        c.execute(
            "INSERT INTO peer_active_users(user_id,session_id)VALUES(1,?1)",
            [created],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        lobby(&a, 2, 102).unwrap()["available"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert!(
        cleanup_candidates(&a, 102)
            .unwrap()
            .iter()
            .any(|row| row.id == host.id)
    );
}

/// The invitation cap is on the invitations a person leaves waiting. A pairing
/// both people confirmed creates a session that is accepted at once, so a host
/// with a full list of waiting invitations must still be able to pair.
#[test]
fn a_full_invitation_list_does_not_block_a_pairing() {
    let a = fixture();
    let config = crate::config::PeerConfig::default();
    for n in 0..super::super::MAX_PENDING_SESSIONS_PER_USER {
        super::super::create(&a, 1, easy(), &config, 100 + n)
            .unwrap()
            .unwrap();
    }
    let host = available(&a, 1, 200);
    set_profile(&a, 2, "Bob", 200).unwrap().unwrap();
    let r = request(&a, 2, &host.id, 201).unwrap().unwrap();
    let id = r["requestId"].as_str().unwrap();
    confirm(&a, 1, id, true, &config, 202).unwrap().unwrap();
    confirm(&a, 2, id, true, &config, 203)
        .unwrap()
        .expect("a pairing is not an invitation left waiting");
}

fn create_on_fixture(a: &Accounts, now: i64) -> String {
    super::super::create(a, 1, easy(), &crate::config::PeerConfig::default(), now)
        .unwrap()
        .unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn an_empty_pending_room_does_not_hold_capacity() {
    let a = fixture();
    let config = crate::config::PeerConfig {
        max_sessions: 1,
        ..Default::default()
    };
    let created = crate::accounts::peer::create(&a, 1, easy(), &config, 100)
        .unwrap()
        .unwrap();
    let id = created["sessionId"].as_str().unwrap().to_owned();
    set_profile(&a, 2, "Bob", 100).unwrap().unwrap();
    let attempt = |now| {
        prepare(
            &a,
            2,
            easy(),
            Some(("interview-wait2", "primary")),
            &config,
            now,
        )
        .unwrap()
    };
    a.with(|c| {
        c.execute(
            "UPDATE peer_sessions SET room_name='r', status='lobby' WHERE id=?1",
            [&id],
        )
    })
    .unwrap();
    assert_eq!(attempt(101).err(), Some("at_capacity"));
    a.with(|c| {
        c.execute(
            "UPDATE peer_sessions SET status='pending' WHERE id=?1",
            [&id],
        )
    })
    .unwrap();
    assert!(
        attempt(102).is_ok(),
        "a session whose members all left is not a live room"
    );
}

fn hold(a: &Accounts, user: i64, session: &str) {
    a.with(|c| {
        c.execute(
            "INSERT INTO peer_active_users(user_id,session_id)VALUES(?1,?2)",
            params![user, session],
        )
    })
    .unwrap();
}

fn private_session(a: &Accounts, user: i64, now: i64) -> String {
    super::super::create(a, user, easy(), &crate::config::PeerConfig::default(), now)
        .unwrap()
        .unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn nickname_length_is_capped_at_twenty_four_bytes() {
    let a = fixture();
    let longest = "a".repeat(24);
    assert_eq!(
        set_profile(&a, 1, &longest, 100).unwrap().unwrap()["nickname"],
        longest.as_str()
    );
    assert_eq!(
        set_profile(&a, 1, &"a".repeat(25), 100)
            .unwrap()
            .unwrap_err(),
        "invalid_nickname"
    );
}

#[test]
fn a_member_in_a_live_session_can_neither_publish_nor_request() {
    let a = fixture();
    let host = available(&a, 1, 100);
    set_profile(&a, 2, "Bob", 100).unwrap().unwrap();
    let session = private_session(&a, 3, 100);
    hold(&a, 2, &session);
    assert_eq!(
        request(&a, 2, &host.id, 101).unwrap().unwrap_err(),
        "already_active"
    );
    assert!(matches!(
        prepare(
            &a,
            2,
            easy(),
            Some(("interview-wait2", "primary")),
            &crate::config::PeerConfig::default(),
            101
        )
        .unwrap(),
        Err("already_active")
    ));
}

#[test]
fn a_request_cannot_target_the_requesters_own_availability() {
    let a = fixture();
    let own = available(&a, 1, 100);
    assert_eq!(
        request(&a, 1, &own.id, 101).unwrap().unwrap_err(),
        "availability_unavailable"
    );
    a.with(|c| {
        let count: i64 =
            c.query_row("SELECT count(*) FROM peer_pair_requests", [], |r| r.get(0))?;
        assert_eq!(count, 0);
        Ok(())
    })
    .unwrap();
}

#[test]
fn one_outgoing_request_at_a_time_and_repeating_it_returns_the_same_one() {
    let a = fixture();
    let first = available(&a, 1, 100);
    set_profile(&a, 3, "Eve", 100).unwrap().unwrap();
    let row = prepare(
        &a,
        3,
        easy(),
        Some(("interview-wait3", "primary")),
        &crate::config::PeerConfig::default(),
        100,
    )
    .unwrap()
    .unwrap();
    let second = publish(&a, &row.id, 3, true, 100).unwrap().unwrap();
    set_profile(&a, 2, "Bob", 100).unwrap().unwrap();
    let made = request(&a, 2, &first.id, 101).unwrap().unwrap();
    assert_eq!(
        request(&a, 2, &second.id, 102).unwrap().unwrap_err(),
        "request_pending"
    );
    let again = request(&a, 2, &first.id, 103).unwrap().unwrap();
    assert_eq!(again["requestId"], made["requestId"]);
}

#[test]
fn each_confirmation_is_recorded_for_the_member_who_gave_it() {
    let a = fixture();
    let host = available(&a, 1, 100);
    set_profile(&a, 2, "Bob", 100).unwrap().unwrap();
    let r = request(&a, 2, &host.id, 101).unwrap().unwrap();
    let id = r["requestId"].as_str().unwrap();
    let config = crate::config::PeerConfig::default();
    let by_requester = confirm(&a, 2, id, true, &config, 102).unwrap().unwrap();
    assert_eq!(by_requester["confirmedByYou"], true);
    assert_eq!(by_requester["confirmedByPartner"], false);
    let by_host_view = lobby(&a, 1, 103).unwrap();
    assert_eq!(by_host_view["requests"][0]["confirmedByYou"], false);
    assert_eq!(by_host_view["requests"][0]["confirmedByPartner"], true);
}

#[test]
fn a_requester_who_became_busy_cannot_complete_a_pairing() {
    let a = fixture();
    let host = available(&a, 1, 100);
    set_profile(&a, 2, "Bob", 100).unwrap().unwrap();
    let r = request(&a, 2, &host.id, 101).unwrap().unwrap();
    let id = r["requestId"].as_str().unwrap();
    let session = private_session(&a, 3, 101);
    hold(&a, 2, &session);
    let config = crate::config::PeerConfig::default();
    assert_eq!(
        confirm(&a, 2, id, true, &config, 102).unwrap().unwrap_err(),
        "already_active"
    );
}

#[test]
fn a_new_pair_session_starts_at_the_version_after_acceptance_and_binding() {
    let a = fixture();
    let host = available(&a, 1, 100);
    set_profile(&a, 2, "Bob", 100).unwrap().unwrap();
    let r = request(&a, 2, &host.id, 101).unwrap().unwrap();
    let id = r["requestId"].as_str().unwrap();
    let config = crate::config::PeerConfig::default();
    confirm(&a, 1, id, true, &config, 102).unwrap().unwrap();
    let paired = confirm(&a, 2, id, true, &config, 103).unwrap().unwrap();
    a.with(|c| {
        let s = load(c, paired["sessionId"].as_str().unwrap())?.unwrap();
        assert_eq!(s.version, 2, "one step to accept, one to bind the room");
        Ok(())
    })
    .unwrap();
}

#[test]
fn declining_closes_a_pending_request_once() {
    let a = fixture();
    let host = available(&a, 1, 100);
    set_profile(&a, 2, "Bob", 100).unwrap().unwrap();
    let r = request(&a, 2, &host.id, 101).unwrap().unwrap();
    let id = r["requestId"].as_str().unwrap();
    assert_eq!(decline(&a, 3, id, 102).unwrap().unwrap_err(), "not_found");
    assert_eq!(
        decline(&a, 1, id, 102).unwrap().unwrap()["status"],
        "declined"
    );
    assert_eq!(
        decline(&a, 1, id, 103).unwrap().unwrap_err(),
        "request_unavailable"
    );
}

#[test]
fn availability_lasts_forty_five_seconds_from_each_refresh() {
    let a = fixture();
    set_profile(&a, 1, "Alice", 100).unwrap().unwrap();
    let config = crate::config::PeerConfig::default();
    let row = prepare(
        &a,
        1,
        easy(),
        Some(("interview-wait1", "primary")),
        &config,
        100,
    )
    .unwrap()
    .unwrap();
    assert_eq!(row.expires_at, 100 + AVAILABILITY_TTL_SECONDS);
    let published = publish(&a, &row.id, 1, true, 110).unwrap().unwrap();
    assert_eq!(published.expires_at, 110 + AVAILABILITY_TTL_SECONDS);
    let refreshed = prepare(&a, 1, easy(), None, &config, 120).unwrap().unwrap();
    assert_eq!(refreshed.expires_at, 120 + AVAILABILITY_TTL_SECONDS);
    assert_eq!(
        lobby(&a, 2, 164).unwrap()["available"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        lobby(&a, 2, 165).unwrap()["available"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn only_the_owner_can_publish_and_only_while_the_entry_is_live() {
    let a = fixture();
    set_profile(&a, 1, "Alice", 100).unwrap().unwrap();
    let config = crate::config::PeerConfig::default();
    let row = prepare(
        &a,
        1,
        easy(),
        Some(("interview-wait1", "primary")),
        &config,
        100,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        publish(&a, &row.id, 2, true, 101).unwrap().err(),
        Some("availability_unavailable")
    );
    leave(&a, 1, 102).unwrap();
    assert_eq!(
        publish(&a, &row.id, 1, true, 103).unwrap().err(),
        Some("availability_unavailable")
    );
}

#[test]
fn a_profile_reports_the_nickname_that_was_saved() {
    let a = fixture();
    assert_eq!(profile(&a, 1).unwrap(), json!({"nickname":null}));
    set_profile(&a, 1, "Alice", 100).unwrap().unwrap();
    assert_eq!(profile(&a, 1).unwrap(), json!({"nickname":"Alice"}));
}

/// One host cannot be asked by more than eight people at once; the ninth is
/// told, and the first eight are unaffected.
#[test]
fn a_ninth_pending_request_to_one_host_is_refused() {
    let a = fixture();
    a.with(|c| {
        c.execute_batch(
            "INSERT INTO users(id,github_id,login,created_at,updated_at) VALUES
             (4,4,'u4',1,1),(5,5,'u5',1,1),(6,6,'u6',1,1),(7,7,'u7',1,1),
             (8,8,'u8',1,1),(9,9,'u9',1,1),(10,10,'u10',1,1)",
        )
    })
    .unwrap();
    let host = available(&a, 1, 200);
    for user in 2..=9 {
        set_profile(&a, user, &format!("Name{user}"), 200)
            .unwrap()
            .unwrap();
        request(&a, user, &host.id, 201)
            .unwrap()
            .unwrap_or_else(|code| panic!("request {user} refused: {code}"));
    }
    set_profile(&a, 10, "Name10", 200).unwrap().unwrap();
    assert_eq!(
        request(&a, 10, &host.id, 201).unwrap().unwrap_err(),
        "request_limit"
    );
}
